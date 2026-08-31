//! Spike S5 — ambiguous-send fault injection.
//!
//! The audit called "a send never duplicates" a blocker, because no mail
//! transport offers idempotent send. This test manufactures the exact failure
//! that makes it hard — **the server commits the message, then the client never
//! hears about it** — and proves the reconciliation rule resolves it correctly.
//!
//! The fault is injected by a proxy that forwards the SMTP conversation
//! faithfully and severs the connection the instant the client finishes the
//! body. GreenMail still receives and delivers the message; our client sees a
//! dead socket. That is not a simulation of the real failure — it *is* the real
//! failure, staged on demand.
//!
//! Needs the test server (testkit/README.md). Run:
//!   cargo test -p petrel-providers --features insecure-plaintext \
//!     --test ambiguous_send -- --ignored --nocapture

#![cfg(feature = "insecure-plaintext")]

use std::time::Duration;

use petrel_engine::outbox::{
    AttemptOutcome, SendState, ServerEvidence, may_retry_automatically, reconcile,
};
use petrel_providers::imap::{Credential, ImapConfig, Security, find_message_id};
use petrel_providers::smtp::{SendResult, send_plaintext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SMTP_HOST: &str = "127.0.0.1";
const SMTP_PORT: u16 = 3025;

fn imap_cfg() -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        port: 3143,
        user: "petrel".into(),
        credential: Credential::password("petrelpass"),
        security: Security::InsecurePlaintext,
    }
}

fn message(message_id: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: Petrel Test <petrel@example.com>\r\n\
         To: petrel@example.com\r\n\
         Subject: {subject}\r\n\
         Date: Thu, 20 Aug 2026 09:00:00 +0000\r\n\
         Message-ID: <{message_id}>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         Ambiguous-send fault injection body.\r\n"
    )
    .into_bytes()
}

/// Forwards SMTP to the real server but cuts the client connection as soon as
/// the client has sent the end-of-DATA marker — after the server has taken the
/// message, before the client can read the acknowledgement.
async fn severing_proxy() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (client, _) = listener.accept().await.expect("accept");
        let upstream = TcpStream::connect((SMTP_HOST, SMTP_PORT))
            .await
            .expect("connect upstream");
        let (mut cr, mut cw) = client.into_split();
        let (mut ur, mut uw) = upstream.into_split();

        // Upstream → client, until we deliberately stop.
        let down = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match ur.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if cw.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Client → upstream, watching for the terminating dot.
        let mut tail = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match cr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if uw.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    let _ = uw.flush().await;
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > 8 {
                        tail.drain(..tail.len() - 8);
                    }
                    if tail.ends_with(b"\r\n.\r\n") {
                        // Order matters. Stop relaying the server's reply FIRST,
                        // so the acknowledgement can never reach the client;
                        // only then wait for the server to actually commit.
                        // Doing it the other way round lets the 250 slip
                        // through and there is no ambiguity to test.
                        down.abort();
                        tokio::time::sleep(Duration::from_millis(600)).await;
                        break;
                    }
                }
            }
        }
        // Dropping both halves closes the client socket mid-acknowledgement.
    });
    port
}

async fn evidence_for(message_id: &str) -> ServerEvidence {
    // GreenMail delivers to the recipient's INBOX; a real account would search
    // Sent/All Mail. Same question either way: does the server have it?
    match find_message_id(&imap_cfg(), "INBOX", message_id).await {
        Ok(hits) if !hits.is_empty() => ServerEvidence::Found,
        Ok(_) => ServerEvidence::Absent,
        Err(_) => ServerEvidence::Indeterminate,
    }
}

fn classify(result: &SendResult) -> AttemptOutcome {
    match result {
        SendResult::Committed { .. } => AttemptOutcome::Accepted,
        SendResult::FailedBeforeCommit { .. } => AttemptOutcome::FailedBeforeCommit,
        SendResult::UnknownAfterTransmit { .. } => AttemptOutcome::UnknownAfterTransmit,
        SendResult::RejectedPermanently { .. } => AttemptOutcome::RejectedPermanently,
    }
}

#[tokio::test]
#[ignore = "requires the local mail test server"]
async fn clean_send_is_committed() {
    let id = "clean-send@petrel.test";
    let result = send_plaintext(
        SMTP_HOST,
        SMTP_PORT,
        "petrel@example.com",
        "petrel@example.com",
        &message(id, "clean send"),
    )
    .await;
    println!("clean send -> {result:?}");
    assert!(matches!(result, SendResult::Committed { .. }));
    assert_eq!(
        reconcile(classify(&result), ServerEvidence::Indeterminate),
        SendState::Sent
    );

    // Baseline for the whole suite: prove mail sent this way is actually
    // findable in the mailbox we search. Without this, an "Absent" verdict in
    // the ambiguous test could mean broken addressing rather than a real
    // rollback — the two look identical from the outside.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let hits = find_message_id(&imap_cfg(), "INBOX", id)
        .await
        .expect("search");
    assert_eq!(
        hits.len(),
        1,
        "a committed send must be findable by Message-ID"
    );
}

#[tokio::test]
#[ignore = "requires the local mail test server"]
async fn connection_refused_is_safe_to_retry() {
    // Nothing can have been committed: no retry hazard.
    let result = send_plaintext(
        SMTP_HOST,
        1, // nothing listens here
        "petrel@example.com",
        "petrel@example.com",
        &message("refused@petrel.test", "refused"),
    )
    .await;
    println!("refused -> {result:?}");
    assert!(matches!(result, SendResult::FailedBeforeCommit { .. }));
    let state = reconcile(classify(&result), ServerEvidence::Absent);
    assert_eq!(state, SendState::RetryQueued);
    assert!(may_retry_automatically(state));
}

/// The one that matters.
#[tokio::test]
#[ignore = "requires the local mail test server"]
async fn severed_after_commit_reconciles_instead_of_duplicating() {
    let id = "severed-send@petrel.test";
    let proxy_port = severing_proxy().await;

    let result = send_plaintext(
        SMTP_HOST,
        proxy_port,
        "petrel@example.com",
        "petrel@example.com",
        &message(id, "severed after commit"),
    )
    .await;
    println!("severed send -> {result:?}");

    // The transport could not tell us what happened.
    assert!(
        matches!(result, SendResult::UnknownAfterTransmit { .. }),
        "severing after the body must produce an ambiguous outcome, got {result:?}"
    );

    // A naive client would now retry and deliver the message twice. Instead we
    // look: give the server a moment, then ask whether it has the message.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let evidence = evidence_for(id).await;
    println!("server evidence for {id}: {evidence:?}");
    assert_eq!(
        evidence,
        ServerEvidence::Found,
        "the server did commit the message despite the client's error"
    );

    let state = reconcile(classify(&result), evidence);
    assert_eq!(state, SendState::Sent, "must not be queued for retry");
    assert!(
        !may_retry_automatically(state),
        "auto-retry here would duplicate the user's mail"
    );

    // And exactly one copy exists.
    let hits = find_message_id(&imap_cfg(), "INBOX", id).await.unwrap();
    println!("copies on server: {}", hits.len());
    assert_eq!(hits.len(), 1, "exactly one copy should exist");
}

#[tokio::test]
#[ignore = "requires the local mail test server"]
async fn ambiguous_and_unverifiable_needs_attention() {
    // Same ambiguous transport outcome, but the mailbox cannot be searched
    // (wrong port stands in for offline/unsearchable). We must not guess.
    let unreachable = ImapConfig {
        port: 1,
        ..imap_cfg()
    };
    let evidence = match find_message_id(&unreachable, "INBOX", "nobody@petrel.test").await {
        Ok(h) if !h.is_empty() => ServerEvidence::Found,
        Ok(_) => ServerEvidence::Absent,
        Err(_) => ServerEvidence::Indeterminate,
    };
    assert_eq!(evidence, ServerEvidence::Indeterminate);

    let state = reconcile(AttemptOutcome::UnknownAfterTransmit, evidence);
    assert_eq!(state, SendState::NeedsAttention);
    assert!(!may_retry_automatically(state));
}
