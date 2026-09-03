//! Submission on 587, against a scripted server.
//!
//! iCloud and Microsoft have nothing listening on 465: an implicit-TLS-only
//! client could read their mail and never send a word of it. The fix is
//! STARTTLS, and the thing worth proving is the order — greeting, EHLO,
//! STARTTLS, upgrade — and that nothing secret is written before the upgrade
//! happens. A loopback socket cannot present a certificate this client would
//! accept, so the conversation is checked up to the handshake and the
//! handshake itself is expected to fail.
//!
//! The refusal case matters as much: a server that offers no STARTTLS must be
//! abandoned, never fallen back on. Falling back is how a password ends up on
//! the wire in front of whoever stripped the capability.

use std::sync::{Arc, Mutex};

use petrel_providers::imap::Credential;
use petrel_providers::smtp::SmtpConfig;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Every line the server was sent, in order.
type Log = Arc<Mutex<Vec<String>>>;

/// A submission server on a random port. `starttls` decides whether it
/// advertises the capability at all.
async fn server(starttls: bool) -> (u16, Log) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        let Ok((sock, _)) = listener.accept().await else {
            return;
        };
        let (rx, mut tx) = sock.into_split();
        let mut reader = BufReader::new(rx);
        let _ = tx.write_all(b"220 mail.example ESMTP\r\n").await;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            seen.lock().unwrap().push(line.trim_end().to_string());
            let upper = line.to_ascii_uppercase();
            let reply: &[u8] = if upper.starts_with("EHLO") {
                if starttls {
                    b"250-mail.example\r\n250-STARTTLS\r\n250-AUTH LOGIN XOAUTH2\r\n250 SIZE 35882577\r\n"
                } else {
                    b"250-mail.example\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 35882577\r\n"
                }
            } else if upper.starts_with("STARTTLS") {
                // Answered, then the socket closes: the client's handshake
                // fails, which is as far as a plaintext script can take it.
                let _ = tx.write_all(b"220 2.0.0 Ready to start TLS\r\n").await;
                return;
            } else if upper.starts_with("QUIT") {
                b"221 bye\r\n"
            } else {
                b"250 ok\r\n"
            };
            if tx.write_all(reply).await.is_err() {
                return;
            }
        }
    });
    (port, log)
}

fn cfg(port: u16) -> SmtpConfig {
    SmtpConfig {
        host: "127.0.0.1".into(),
        port,
        user: "someone@example.com".into(),
        credential: Credential::password("hunter2-not-a-real-password"),
    }
}

#[tokio::test]
async fn a_587_server_is_greeted_upgraded_and_only_then_signed_in_to() {
    let (port, log) = server(true).await;
    let err = petrel_providers::smtp::login_check(&cfg(port))
        .await
        .expect_err("a loopback socket has no certificate we would accept");

    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen.first().map(String::as_str),
        Some("EHLO [127.0.0.1]"),
        "EHLO names the client, not the server: {seen:?}"
    );
    assert_eq!(
        seen.get(1).map(String::as_str),
        Some("STARTTLS"),
        "the upgrade is asked for straight after EHLO: {seen:?}"
    );
    // The password, and anything that could carry it, stays unwritten until
    // the socket is encrypted — which here never happens.
    assert!(
        !seen
            .iter()
            .any(|l| l.to_ascii_uppercase().starts_with("AUTH")),
        "nothing was authenticated in the clear: {seen:?}"
    );
    assert!(
        !seen.iter().any(|l| l.contains("hunter2")),
        "the password never reached the wire: {seen:?}"
    );
    assert!(
        err.contains("starttls"),
        "the failure is the handshake, not the conversation: {err}"
    );
}

#[tokio::test]
async fn a_server_offering_no_starttls_is_refused_rather_than_downgraded() {
    let (port, log) = server(false).await;
    let err = petrel_providers::smtp::login_check(&cfg(port))
        .await
        .expect_err("submission in the clear is not an option");

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen, vec!["EHLO [127.0.0.1]".to_string()], "{seen:?}");
    assert!(
        err.to_ascii_lowercase().contains("starttls"),
        "the reason is said plainly: {err}"
    );
}
