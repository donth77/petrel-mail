//! Two folder-level commands, and what the server's answer means.
//!
//! A CREATE that failed used to be read as success whenever the error
//! mentioned the word "exist" — which "mailbox does not exist" does, so a
//! folder that could not be made for want of its parent was reported as made.
//!
//! And the placement sweep asked one `UID SEARCH ALL` per folder per cycle.
//! On a Gmail All Mail holding three hundred thousand messages that is
//! twenty-one seconds of somebody's server, every twenty minutes, for as long
//! as the backfill runs. Ranges are answered from the UID index instead.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{Credential, ImapConfig, Security, create_folder, uids_in_folder};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type Log = Arc<Mutex<Vec<String>>>;

/// A server holding `uid_next - 1` messages, answering CREATE with `create`.
async fn server(uid_next: u32, create: &'static str) -> (u16, Log) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
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
                    seen.lock().unwrap().push(line.trim_end().to_string());
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    let upper = line.to_ascii_uppercase();
                    let reply = if upper.contains(" CREATE ") {
                        format!("{tag} {create}\r\n")
                    } else if upper.contains(" SELECT ") || upper.contains(" EXAMINE ") {
                        format!(
                            "* {} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {uid_next}] ok\r\n{tag} OK done\r\n",
                            uid_next - 1
                        )
                    } else if upper.contains(" UID SEARCH ") {
                        // Answers with the first UID of whatever range it was
                        // asked about, which is enough to show the ranges.
                        let first: u32 = line
                            .split_whitespace()
                            .nth(4)
                            .and_then(|spec| spec.split(':').next()?.parse().ok())
                            .unwrap_or(1);
                        format!("* SEARCH {first}\r\n{tag} OK searched\r\n")
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
    (port, log)
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
async fn a_create_succeeds_only_when_the_folder_is_really_there() {
    for answer in [
        "OK created",
        "NO [ALREADYEXISTS] Mailbox already exists",
        "NO Mailbox already exists.",
    ] {
        let (port, _) = server(2, answer).await;
        create_folder(&cfg(port), "Receipts")
            .await
            .unwrap_or_else(|e| panic!("{answer} means the folder is there, got {e}"));
    }
    // The word "exist" in a refusal is not a folder. Both of these used to
    // report success and leave the caller filing mail into nothing.
    for answer in [
        "NO Mailbox does not exist",
        "NO [CANNOT] Parent mailbox does not exist",
        "BAD Command Argument Error",
    ] {
        let (port, _) = server(2, answer).await;
        create_folder(&cfg(port), "Receipts/2026")
            .await
            .expect_err(answer);
    }
}

#[tokio::test]
async fn the_uid_sweep_asks_in_ranges_rather_than_about_everything_at_once() {
    // 120,001 messages: three ranges of fifty thousand, and nothing bigger.
    let (port, log) = server(120_002, "OK created").await;
    let uids = uids_in_folder(&cfg(port), "[Gmail]/All Mail")
        .await
        .expect("swept");
    assert_eq!(uids, vec![1, 50_001, 100_001], "one answer per range");

    let searches: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.to_ascii_uppercase().contains("UID SEARCH"))
        .cloned()
        .collect();
    assert_eq!(searches.len(), 3, "{searches:?}");
    assert!(
        searches
            .iter()
            .all(|s| !s.to_ascii_uppercase().contains("ALL")),
        "never the whole mailbox at once: {searches:?}"
    );
    assert!(
        searches[2].contains("100001:120001"),
        "the last range stops at the last UID: {searches:?}"
    );
}
