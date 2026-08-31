//! UIDVALIDITY reset, proven end to end against a scripted server.
//!
//! Self-contained: an in-process IMAP server renumbers its folder between
//! phases, and the test drives the same three calls the app's sync loop
//! composes — watermark fetch, id map, uid-set refetch — against a real
//! store. No container, no network, not ignored: this is the regression
//! fence for "a reset costs at worst a re-download, never data".
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;
use petrel_providers::imap::{Credential, FetchOutcome, ImapConfig, Security};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Msg {
    uid: u32,
    mid: &'static str,
    raw: String,
}

fn msg(uid: u32, mid: &'static str, subject: &str) -> Msg {
    Msg {
        uid,
        mid,
        raw: format!(
            "From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\n\
             Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
             Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\nbody of {subject}\r\n"
        ),
    }
}

/// The folder as the server currently tells it.
struct MailboxState {
    validity: u32,
    messages: Vec<Msg>,
}

/// Speaks just enough IMAP for login, select, and the three fetch shapes the
/// client uses. Each provider call opens a fresh connection, so the server
/// accepts sessions in a loop until dropped.
async fn scripted_server(state: Arc<Mutex<MailboxState>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK petrel-scripted ready\r\n").await;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    let upper = line.to_ascii_uppercase();
                    let mut out: Vec<u8> = Vec::new();
                    if upper.contains(" LOGIN ") {
                        out.extend_from_slice(format!("{tag} OK logged in\r\n").as_bytes());
                    } else if upper.contains(" SELECT ") {
                        let s = state.lock().unwrap();
                        out.extend_from_slice(
                            format!(
                                "* {} EXISTS\r\n* 0 RECENT\r\n\
                                 * OK [UIDVALIDITY {}] UIDs valid\r\n\
                                 * FLAGS (\\Seen \\Flagged)\r\n\
                                 {tag} OK [READ-WRITE] selected\r\n",
                                s.messages.len(),
                                s.validity
                            )
                            .as_bytes(),
                        );
                    } else if upper.contains("HEADER.FIELDS (MESSAGE-ID)") {
                        // Sequence fetch of id headers: every message, in order.
                        let s = state.lock().unwrap();
                        for (i, m) in s.messages.iter().enumerate() {
                            let hdr = format!("Message-ID: <{}>\r\n\r\n", m.mid);
                            out.extend_from_slice(
                                format!(
                                    "* {} FETCH (UID {} BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n{hdr})\r\n",
                                    i + 1,
                                    m.uid,
                                    hdr.len()
                                )
                                .as_bytes(),
                            );
                        }
                        out.extend_from_slice(format!("{tag} OK fetched\r\n").as_bytes());
                    } else if upper.contains("UID FETCH") {
                        // Two shapes: a watermark range "N:*" and an explicit set.
                        let s = state.lock().unwrap();
                        let spec = line.split_whitespace().nth(3).unwrap_or("1:*").to_string();
                        let wanted: Vec<&Msg> = if let Some((start, _)) = spec.split_once(":") {
                            let start: u32 = start.parse().unwrap_or(1);
                            // RFC semantics: a start past the end clamps to the
                            // last message, which is exactly the trap a reset
                            // sets for watermark fetches.
                            if start > s.messages.iter().map(|m| m.uid).max().unwrap_or(0) {
                                s.messages.iter().rev().take(1).collect()
                            } else {
                                s.messages.iter().filter(|m| m.uid >= start).collect()
                            }
                        } else {
                            let set: Vec<u32> =
                                spec.split(',').filter_map(|u| u.parse().ok()).collect();
                            s.messages.iter().filter(|m| set.contains(&m.uid)).collect()
                        };
                        for (i, m) in wanted.iter().enumerate() {
                            out.extend_from_slice(
                                format!(
                                    "* {} FETCH (UID {} FLAGS (\\Seen) RFC822 {{{}}}\r\n{})\r\n",
                                    i + 1,
                                    m.uid,
                                    m.raw.len(),
                                    m.raw
                                )
                                .as_bytes(),
                            );
                        }
                        out.extend_from_slice(format!("{tag} OK fetched\r\n").as_bytes());
                    } else if upper.contains(" LOGOUT") {
                        out.extend_from_slice(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes());
                        let _ = tx.write_all(&out).await;
                        return;
                    } else {
                        out.extend_from_slice(format!("{tag} OK noop\r\n").as_bytes());
                    }
                    if tx.write_all(&out).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn a_renumbered_folder_is_remapped_without_losing_mail() {
    // Phase one: three messages under validity 111, UIDs in the hundreds.
    let mailbox = Arc::new(Mutex::new(MailboxState {
        validity: 111,
        messages: vec![
            msg(101, "alpha@x", "alpha"),
            msg(102, "beta@x", "beta"),
            msg(103, "gamma@x", "gamma"),
        ],
    }));
    let port = scripted_server(Arc::clone(&mailbox)).await;
    let cfg = ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "petrel".into(),
        credential: Credential::password("petrelpass"),
        security: Security::InsecurePlaintext,
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let folder = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("folder");

    // First sync: watermark 0, no expected validity — adopt the server's.
    let store_cell = Mutex::new(&mut store);
    let outcome =
        petrel_providers::imap::fetch_since_each(&cfg, "INBOX", 0, None, |uid, _f, raw| {
            let mut s = store_cell.lock().unwrap();
            s.ingest_raw(&blobs, account, Some(folder), Some(uid), raw)
                .expect("ingest");
        })
        .await
        .expect("first sync");
    let FetchOutcome::Fetched {
        count,
        uid_validity,
    } = outcome
    else {
        panic!("first sync must fetch: {outcome:?}");
    };
    assert_eq!(count, 3);
    assert_eq!(uid_validity, Some(111));
    store.set_folder_validity(folder, uid_validity).unwrap();
    assert_eq!(store.max_uid(folder).unwrap(), Some(103));

    // The reset: renumbered from 1, beta gone, delta new.
    *mailbox.lock().unwrap() = MailboxState {
        validity: 222,
        messages: vec![
            msg(1, "alpha@x", "alpha"),
            msg(3, "gamma@x", "gamma"),
            msg(4, "delta@x", "delta"),
        ],
    };

    // The next watermark pass must refuse to fetch — a `104:*` fetch against
    // the new numbering would clamp to the last message and quietly misfile
    // it, which is the exact trap the outcome exists to catch.
    let expected = store.folder_validity(folder).unwrap();
    let polled = petrel_providers::imap::fetch_since_each(
        &cfg,
        "INBOX",
        store.max_uid(folder).unwrap().unwrap_or(0),
        expected,
        |_, _, _| panic!("nothing may be fetched across a validity change"),
    )
    .await
    .expect("poll");
    assert_eq!(polled, FetchOutcome::ValidityChanged { now: Some(222) });

    // Recovery, composed exactly as the app composes it.
    let map = petrel_providers::imap::fetch_id_map(&cfg, "INBOX", 200)
        .await
        .expect("id map");
    assert_eq!(map.uid_validity, Some(222));
    assert!(map.complete);
    let remap = store
        .remap_folder_after_reset(folder, &map.entries, map.complete)
        .expect("remap");
    assert_eq!(remap.rematched, 2, "alpha and gamma keep their history");
    assert_eq!(remap.dropped, 1, "beta left the folder");
    assert_eq!(remap.to_fetch, vec![4], "only delta is re-downloaded");

    let store_cell = Mutex::new(&mut store);
    let refetched =
        petrel_providers::imap::fetch_uids_each(&cfg, "INBOX", &remap.to_fetch, |uid, _f, raw| {
            let mut s = store_cell.lock().unwrap();
            s.ingest_raw(&blobs, account, Some(folder), Some(uid), raw)
                .expect("ingest");
        })
        .await
        .expect("refetch");
    assert_eq!(refetched, 1);
    store.set_folder_validity(folder, map.uid_validity).unwrap();

    // The mend, checked: new numbering live, nothing duplicated, and the
    // next watermark pass runs clean under the new validity.
    assert_eq!(store.max_uid(folder).unwrap(), Some(4));
    let final_pass = petrel_providers::imap::fetch_since_each(
        &cfg,
        "INBOX",
        4,
        store.folder_validity(folder).unwrap(),
        |_, _, _| panic!("nothing new to fetch"),
    )
    .await
    .expect("clean pass");
    assert!(
        matches!(final_pass, FetchOutcome::Fetched { count: 0, .. }),
        "{final_pass:?}"
    );
}
