//! A FETCH that ends badly must not count as a FETCH that ended.
//!
//! The typed fetch stream stops at the tagged reply without reading its
//! status, so "OK" and "NO [SERVERBUG] internal error" arrived identically —
//! as a stream that simply ends. The pass then recorded UIDNEXT, and the mail
//! the server had refused to send sat above the watermark, never asked for
//! again. Gmail says "Some messages could not be FETCHed", Dovecot says
//! SERVERBUG, Exchange says "BAD Command Argument Error"; all three took this
//! path.
//!
//! The same rule covers the quieter version: an item the server lists and then
//! gives no body for. Skipping it silently while the watermark moves past it
//! loses exactly one message, which is the kind of loss nobody reports.
#![cfg(feature = "insecure-plaintext")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, FolderPass, ImapConfig, PassOutcome, Security, sync_pass,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// What the server does with one command line.
enum Reply {
    Bytes(Vec<u8>),
    /// Write these bytes and hang up.
    CloseAfter(Vec<u8>),
}

fn bytes(s: String) -> Reply {
    Reply::Bytes(s.into_bytes())
}

fn message(uid: u32) -> String {
    format!(
        "From: a@example.com\r\nTo: b@example.com\r\nSubject: m{uid}\r\n\
         Message-ID: <m{uid}@x>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain\r\n\r\nbody {uid}\r\n"
    )
}

/// One `* n FETCH (...)` line carrying a whole message.
fn fetch_line(seq: u32, uid: u32) -> String {
    let raw = message(uid);
    format!(
        "* {seq} FETCH (UID {uid} FLAGS (\\Seen) BODY[] {{{}}}\r\n{raw})\r\n",
        raw.len()
    )
}

/// LOGIN, CAPABILITY, STATUS, EXAMINE and LOGOUT, answered as a server would.
fn baseline(tag: &str, line: &str, exists: u32, uid_next: u32) -> Option<Reply> {
    let upper = line.to_ascii_uppercase();
    if upper.contains(" LOGIN ") {
        return Some(bytes(format!("{tag} OK signed in\r\n")));
    }
    if upper.contains(" CAPABILITY") {
        return Some(bytes(format!(
            "* CAPABILITY IMAP4rev1 CONDSTORE UIDPLUS\r\n{tag} OK done\r\n"
        )));
    }
    if upper.contains(" STATUS ") {
        return Some(bytes(format!(
            "* STATUS \"INBOX\" (MESSAGES {exists} UIDNEXT {uid_next} UIDVALIDITY 1 HIGHESTMODSEQ 7)\r\n{tag} OK done\r\n"
        )));
    }
    if upper.contains(" EXAMINE ") || upper.contains(" SELECT ") {
        return Some(bytes(format!(
            "* {exists} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {uid_next}] ok\r\n\
             * OK [HIGHESTMODSEQ 7] ok\r\n{tag} OK [READ-ONLY] done\r\n"
        )));
    }
    if upper.contains(" LOGOUT") {
        return Some(Reply::CloseAfter(
            format!("* BYE\r\n{tag} OK bye\r\n").into_bytes(),
        ));
    }
    None
}

async fn server<F>(handler: F) -> u16
where
    F: Fn(&str, &str) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    match handler(&tag, &line) {
                        Reply::Bytes(b) => {
                            if tx.write_all(&b).await.is_err() {
                                return;
                            }
                        }
                        Reply::CloseAfter(b) => {
                            let _ = tx.write_all(&b).await;
                            let _ = tx.shutdown().await;
                            return;
                        }
                    }
                }
            });
        }
    });
    port
}

fn cfg(port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p"),
        security: Security::InsecurePlaintext,
    }
}

fn pass(since_uid: u32, since_uidnext: Option<u32>) -> Vec<FolderPass> {
    vec![FolderPass {
        path: "INBOX".into(),
        since_uid,
        expected_validity: Some(1),
        since_uidnext,
        since_modseq: Some(7),
        seed_window: 50,
    }]
}

/// Runs one pass and reports the outcome alongside the UIDs it ingested.
async fn run(port: u16, passes: Vec<FolderPass>) -> (PassOutcome, Vec<u32>) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&got);
    let mut outcomes = sync_pass(&cfg(port), &passes, false, move |_, uid, _, raw| {
        assert!(!raw.is_empty(), "an empty body must never be ingested");
        seen.lock().unwrap().push(uid);
    })
    .await
    .expect("the pass itself completes");
    let ingested = got.lock().unwrap().clone();
    (outcomes.remove(0), ingested)
}

#[tokio::test]
async fn a_tagged_no_after_a_partial_seed_records_nothing() {
    // Four messages exist. The server sends one and then refuses.
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let port = server(move |tag, line| {
        if let Some(r) = baseline(tag, line, 4, 5) {
            return r;
        }
        if !line.to_ascii_uppercase().contains("FETCH") {
            return bytes(format!("{tag} OK done\r\n"));
        }
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            return bytes(format!(
                "{}{tag} NO [SERVERBUG] internal error\r\n",
                fetch_line(1, 1)
            ));
        }
        // The next cycle: the server has recovered.
        let mut out = String::new();
        for uid in 1..=4 {
            out.push_str(&fetch_line(uid, uid));
        }
        bytes(format!("{out}{tag} OK fetched\r\n"))
    })
    .await;

    let (outcome, ingested) = run(port, pass(0, None)).await;
    match outcome {
        PassOutcome::Failed { detail } => {
            assert!(detail.contains("No"), "the status is reported: {detail}");
        }
        other => panic!("a refused FETCH is a failed folder, not a short one: {other:?}"),
    }
    assert_eq!(
        ingested,
        vec![1],
        "what did arrive is still handed on; what matters is the watermark"
    );

    // Nothing was recorded, so the next cycle starts where the last one did —
    // and 2, 3 and 4 are asked for rather than skipped for good.
    let (outcome, ingested) = run(port, pass(0, None)).await;
    assert!(
        matches!(outcome, PassOutcome::Fetched { fetched: 4, .. }),
        "{outcome:?}"
    );
    assert_eq!(ingested, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn a_catch_up_slice_answered_no_leaves_the_watermark_where_it_was() {
    // since_uid 100 against UIDNEXT 600: slices 101:300, 301:500, 501:599.
    // The second one is refused.
    let slices = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&slices);
    let port = server(move |tag, line| {
        if let Some(r) = baseline(tag, line, 500, 600) {
            return r;
        }
        if !line.to_ascii_uppercase().contains("FETCH") {
            return bytes(format!("{tag} OK done\r\n"));
        }
        let first: u32 = line
            .split_whitespace()
            .nth(3)
            .and_then(|spec| spec.split(':').next()?.parse().ok())
            .unwrap_or(1);
        if counter.fetch_add(1, Ordering::SeqCst) == 1 {
            return bytes(format!("{tag} NO temporary failure\r\n"));
        }
        bytes(format!("{}{tag} OK fetched\r\n", fetch_line(1, first)))
    })
    .await;

    let (outcome, ingested) = run(port, pass(100, Some(101))).await;
    match outcome {
        PassOutcome::Failed { detail } => assert!(detail.contains("No"), "{detail}"),
        other => panic!("the slice was not delivered, so the folder failed: {other:?}"),
    }
    assert_eq!(ingested, vec![101], "only the slice that arrived");
}

#[tokio::test]
async fn a_server_that_hangs_up_mid_fetch_records_nothing() {
    let port = server(|tag, line| {
        if let Some(r) = baseline(tag, line, 4, 5) {
            return r;
        }
        if line.to_ascii_uppercase().contains("FETCH") {
            return Reply::CloseAfter(
                format!("{}* BYE going down\r\n", fetch_line(1, 1)).into_bytes(),
            );
        }
        bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    let outcome = sync_pass(&cfg(port), &pass(0, None), false, |_, _, _, _| {}).await;
    // Either shape is correct: the folder failed, or the whole pass did on the
    // dead socket. What must not happen is a watermark being recorded.
    if let Ok(outcomes) = outcome {
        assert!(
            matches!(outcomes.first(), Some(PassOutcome::Failed { .. })),
            "{outcomes:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_message_with_no_body_holds_the_watermark_below_it() {
    // Three messages; the server gives uid 2 as NIL — Exchange does this on a
    // message it cannot read back.
    let port = server(|tag, line| {
        if let Some(r) = baseline(tag, line, 3, 4) {
            return r;
        }
        if line.to_ascii_uppercase().contains("FETCH") {
            return bytes(format!(
                "{}* 2 FETCH (UID 2 FLAGS () BODY[] NIL)\r\n{}{tag} OK fetched\r\n",
                fetch_line(1, 1),
                fetch_line(3, 3)
            ));
        }
        bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    let (outcome, ingested) = run(port, pass(0, None)).await;
    let PassOutcome::Fetched {
        fetched, uid_next, ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(fetched, 2, "the empty one is not a message");
    assert_eq!(ingested, vec![1, 3]);
    assert_eq!(
        uid_next,
        Some(2),
        "the watermark stops below the message that was never delivered"
    );
}

#[tokio::test]
async fn an_item_with_no_uid_stops_the_watermark_moving_at_all() {
    // Exchange has been seen to omit UID from a UID FETCH response. There is
    // no number to hold the watermark at, so it does not move.
    let port = server(|tag, line| {
        if let Some(r) = baseline(tag, line, 2, 103) {
            return r;
        }
        if line.to_ascii_uppercase().contains("FETCH") {
            let raw = message(101);
            return bytes(format!(
                "* 1 FETCH (FLAGS () BODY[] {{{}}}\r\n{raw})\r\n{tag} OK fetched\r\n",
                raw.len()
            ));
        }
        bytes(format!("{tag} OK done\r\n"))
    })
    .await;

    let (outcome, ingested) = run(port, pass(100, Some(101))).await;
    let PassOutcome::Fetched { uid_next, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(ingested.is_empty(), "an item with no UID cannot be placed");
    assert_eq!(
        uid_next, None,
        "nothing is recorded, so the range is asked for again"
    );
}
