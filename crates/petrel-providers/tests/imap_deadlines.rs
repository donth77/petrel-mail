//! A server that stops talking must not stop the account.
//!
//! No IMAP conversation had a deadline of any kind. A server that answered
//! LOGIN, STATUS and EXAMINE and then never answered the FETCH left the pass
//! pending for as long as the process lived — and a watcher whose DONE went
//! unanswered sat there holding a wake it could never deliver. Both are the
//! everyday consequence of a closed lid or a NAT mapping expiring, and both
//! present as an account that has simply gone quiet: no error, no retry,
//! nothing to notice.
//!
//! Own file, so the deadlines it shortens cannot race another test's.
#![cfg(feature = "insecure-plaintext")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use petrel_providers::imap::{
    Credential, FolderPass, ImapConfig, PassOutcome, Security, idle_watch, sync_pass,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Seconds the tests give each deadline. Long enough not to fire by accident
/// on a loaded machine, short enough that a hang is obvious.
fn shorten_the_deadlines() {
    // Safety: these are read through `std::env::var` on the same values by
    // every test in this binary, so a race between them changes nothing.
    unsafe {
        std::env::set_var("PETREL_IMAP_READ_SECONDS", "2");
        std::env::set_var("PETREL_IMAP_IDLE_READ_SECONDS", "4");
        std::env::set_var("PETREL_IMAP_COMMAND_SECONDS", "2");
    }
}

/// A server that answers everything up to `mute`, and then says nothing.
async fn server(mute: &'static str) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let muted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&muted);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let counter = Arc::clone(&counter);
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
                    let upper = line.to_ascii_uppercase();
                    if upper.contains(mute) {
                        // The whole point: the socket stays open and nothing
                        // else is ever written to it.
                        counter.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                    let reply = if upper.contains(" CAPABILITY") {
                        format!("* CAPABILITY IMAP4rev1 IDLE CONDSTORE\r\n{tag} OK done\r\n")
                    } else if upper.contains(" STATUS ") {
                        // Named as asked: a STATUS answered under another
                        // name is one the client discards, and that would
                        // look like a reset rather than an answer.
                        let asked = line
                            .split_whitespace()
                            .nth(2)
                            .map(|w| w.trim_matches('"').to_string())
                            .unwrap_or_else(|| "INBOX".to_string());
                        format!(
                            "* STATUS \"{asked}\" (MESSAGES 3 UIDNEXT 4 UIDVALIDITY 1 HIGHESTMODSEQ 7)\r\n{tag} OK done\r\n"
                        )
                    } else if upper.contains(" EXAMINE ") || upper.contains(" SELECT ") {
                        format!(
                            "* 3 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 4] ok\r\n{tag} OK done\r\n"
                        )
                    } else if upper.starts_with("DONE") {
                        // Answered only when DONE is not what this server
                        // went quiet on.
                        format!("{tag} OK idle done\r\n")
                    } else if upper.contains(" IDLE") {
                        let _ = tx.write_all(b"+ idling\r\n").await;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        "* 5 EXISTS\r\n".to_string()
                    } else {
                        format!("{tag} OK done\r\n")
                    };
                    if tx.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, muted)
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

#[tokio::test]
async fn a_pass_whose_fetch_is_never_answered_gives_up() {
    shorten_the_deadlines();
    let (port, muted) = server("FETCH").await;
    let passes = vec![FolderPass {
        path: "INBOX".into(),
        since_uid: 0,
        expected_validity: Some(1),
        since_uidnext: None,
        since_modseq: None,
        seed_window: 50,
    }];

    let started = Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        sync_pass(&cfg(port), &passes, false, |_, _, _, _| {}),
    )
    .await
    .expect("the pass must not outlive the test");

    assert_eq!(muted.load(Ordering::SeqCst), 1, "the FETCH was reached");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it gave up on its own deadline, not the test's: {:?}",
        started.elapsed()
    );
    let outcomes = outcome.expect("the pass reports rather than failing whole");
    match outcomes.first() {
        Some(PassOutcome::Failed { detail }) => assert!(
            detail.contains("said nothing"),
            "and says why, so the log is worth reading: {detail}"
        ),
        other => panic!("a folder that never answered is a failed folder: {other:?}"),
    }
}

#[tokio::test]
async fn a_watch_whose_done_is_never_answered_ends() {
    shorten_the_deadlines();
    // Wakes on an EXISTS, then never answers the DONE that has to come before
    // the wake can be handed on.
    let (port, muted) = server("DONE").await;
    let woke = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woke);

    let started = Instant::now();
    let watch = tokio::time::timeout(
        Duration::from_secs(30),
        idle_watch(&cfg(port), "INBOX", Duration::from_secs(600), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .await
    .expect("the watch must not outlive the test");

    assert_eq!(muted.load(Ordering::SeqCst), 1, "DONE was sent");
    assert!(
        watch.is_err(),
        "a watch that cannot leave IDLE is over, not waiting: {watch:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "and it ended on its own deadline: {:?}",
        started.elapsed()
    );
    assert_eq!(
        woke.load(Ordering::SeqCst),
        0,
        "the wake is never delivered from a session still in IDLE"
    );
}

/// A socket that dies in the middle of a pass takes the session with it,
/// and every folder after that point must say so — not answer as though the
/// server had spoken. The stream yields nothing forever once it has closed,
/// so a STATUS asked on it comes back empty, and an empty STATUS names no
/// UIDVALIDITY: read as a reset, that sent every remaining folder into a
/// re-mapping that stripped the server numbers from all but its newest
/// messages. A lid closed mid-pass was enough to trigger it.
#[tokio::test]
async fn a_socket_that_dies_mid_pass_fails_the_remaining_folders_rather_than_resetting_them() {
    shorten_the_deadlines();
    let (port, muted) = server("FETCH").await;
    let passes: Vec<FolderPass> = ["INBOX", "Archive", "Sent", "Receipts"]
        .iter()
        .map(|path| FolderPass {
            path: path.to_string(),
            since_uid: 0,
            expected_validity: Some(1),
            since_uidnext: None,
            since_modseq: None,
            seed_window: 50,
        })
        .collect();

    let outcomes = tokio::time::timeout(
        Duration::from_secs(30),
        sync_pass(&cfg(port), &passes, false, |_, _, _, _| {}),
    )
    .await
    .expect("the pass must not outlive the test")
    .expect("the pass reports rather than failing whole");

    assert_eq!(
        muted.load(Ordering::SeqCst),
        1,
        "the first FETCH was reached"
    );
    assert_eq!(
        outcomes.len(),
        4,
        "every folder gets a verdict: {outcomes:?}"
    );
    for (i, outcome) in outcomes.iter().enumerate() {
        match outcome {
            PassOutcome::Failed { detail } if i == 0 => {
                assert!(detail.contains("said nothing"), "{detail}")
            }
            PassOutcome::Failed { detail } => assert!(
                detail.contains("connection was lost"),
                "folder {i} was never reached, and says so: {detail}"
            ),
            other => panic!("folder {i} must fail, not report a reset or a fetch: {other:?}"),
        }
    }
}
