//! A server that stops talking must not stop the client.
//!
//! Nothing in the SMTP conversation had a deadline, so a server that accepted
//! the connection and then went quiet left the client awaiting a reply for
//! good. Sending is awaited inside the same worker that delivers queued triage,
//! so one hung send stopped archives and moves going out too — with no error,
//! no retry, and no symptom beyond mail that never left.
//!
//! Both tests here stage exactly that, against fake servers that answer
//! correctly right up to the point where they fall silent.
//!
//! What they cover is the deadline policy and, more importantly, how a timeout
//! is *classified*: before the body is committed a timeout is a plain failure
//! and safe to retry; after it, the server may already have taken the message,
//! so the outcome has to be the ambiguous one that is never retried
//! automatically. Getting that backwards would send somebody's message twice.
//!
//! These drive `send_plaintext` because a loopback socket cannot present a
//! certificate the shipping client would accept. It is the same policy, read
//! from the same four knobs as `send_tls`.
//!
//! Own file, so the environment it sets cannot race another test's.

use std::time::{Duration, Instant};

use petrel_providers::smtp::{SendResult, send_plaintext};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

fn message() -> Vec<u8> {
    b"From: a@example.com\r\nTo: b@example.com\r\nSubject: hi\r\n\r\nbody\r\n".to_vec()
}

/// Accepts the connection and says nothing at all, ever.
async fn mute_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        // Held, not dropped: dropping the socket would give the client a clean
        // EOF, which is a different failure with a different answer.
        let (socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(socket);
    });
    port
}

/// Speaks correctly through to the DATA go-ahead, swallows the body, then
/// falls silent instead of acknowledging.
async fn silent_after_body() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (rx, mut tx) = socket.into_split();
        let mut reader = BufReader::new(rx);
        let mut line = String::new();
        tx.write_all(b"220 fake ready\r\n").await.unwrap();
        for reply in [
            &b"250-fake\r\n250 OK\r\n"[..],
            &b"250 OK\r\n"[..],
            &b"250 OK\r\n"[..],
            &b"354 go ahead\r\n"[..],
        ] {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            tx.write_all(reply).await.unwrap();
        }
        // Read the body to its terminating dot, so the message really has been
        // handed over — then say nothing. This is the dangerous case: from the
        // client's side, delivered and not delivered look identical.
        loop {
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            if line.trim_end() == "." {
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(tx);
    });
    port
}

#[tokio::test]
async fn a_server_that_never_greets_fails_rather_than_hangs() {
    unsafe { std::env::set_var("PETREL_SMTP_REPLY_SECONDS", "2") };
    let port = mute_server().await;
    let started = Instant::now();
    let result = send_plaintext(
        "127.0.0.1",
        port,
        "a@example.com",
        "b@example.com",
        &message(),
    )
    .await;

    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the send must give up, not wait for the server's lifetime"
    );
    match result {
        // Nothing was transmitted, so this one is safe to try again.
        SendResult::FailedBeforeCommit { stage, detail } => {
            assert_eq!(stage, "greeting");
            assert!(detail.contains("timed out"), "said: {detail}");
        }
        other => panic!("expected a failure before commit, got {other:?}"),
    }
}

#[tokio::test]
async fn a_server_that_goes_quiet_after_the_body_is_ambiguous_not_failed() {
    unsafe { std::env::set_var("PETREL_SMTP_COMMIT_SECONDS", "2") };
    let port = silent_after_body().await;
    let started = Instant::now();
    let result = send_plaintext(
        "127.0.0.1",
        port,
        "a@example.com",
        "b@example.com",
        &message(),
    )
    .await;

    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the send must give up, not wait for the server's lifetime"
    );
    match result {
        // The whole point. The server has the message; only the answer is
        // missing. Calling this a failure would let the retry ladder send it a
        // second time, which is the one outcome mail cannot take back.
        SendResult::UnknownAfterTransmit { detail } => {
            assert!(detail.contains("timed out"), "said: {detail}");
        }
        other => panic!("a timeout after the body must be ambiguous, got {other:?}"),
    }
}
