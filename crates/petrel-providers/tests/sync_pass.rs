//! The one-connection sync cycle, proven against a scripted server.
//!
//! The claims that matter: a quiet folder costs one STATUS line and is never
//! selected or fetched; a folder with new mail fetches only above the
//! watermark; the whole pass uses a single login; a renumbered folder is
//! reported, not fetched; flag changes arrive as a CONDSTORE diff.
#![cfg(feature = "insecure-plaintext")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, FolderPass, ImapConfig, PassOutcome, Security, sync_pass,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Msg {
    uid: u32,
    flags: &'static str,
    raw: String,
}

struct Folder {
    validity: u32,
    modseq: u64,
    messages: Vec<Msg>,
}

struct ServerState {
    folders: std::collections::HashMap<String, Folder>,
    logins: AtomicUsize,
    selects: AtomicUsize,
    fetches: AtomicUsize,
}

fn msg(uid: u32, subject: &str) -> Msg {
    Msg {
        uid,
        flags: "\\Seen",
        raw: format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: {subject}\r\n\
             Message-ID: <{subject}@x>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain\r\n\r\nbody {subject}\r\n"
        ),
    }
}

async fn server(state: Arc<Mutex<ServerState>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
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
                let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                let mut line = String::new();
                let mut selected: Option<String> = None;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    let upper = line.to_ascii_uppercase();
                    let mut out: Vec<u8> = Vec::new();
                    let unq = |s: &str| s.trim_matches('"').to_string();
                    if upper.contains(" LOGIN ") {
                        state.lock().unwrap().logins.fetch_add(1, Ordering::Relaxed);
                        out.extend(format!("{tag} OK in\r\n").bytes());
                    } else if upper.contains(" CAPABILITY") {
                        out.extend(b"* CAPABILITY IMAP4rev1 CONDSTORE\r\n".iter());
                        out.extend(format!("{tag} OK done\r\n").bytes());
                    } else if upper.contains(" STATUS ") {
                        let name = unq(line.split_whitespace().nth(2).unwrap_or(""));
                        let s = state.lock().unwrap();
                        match s.folders.get(&name) {
                            Some(f) => {
                                let next = f.messages.iter().map(|m| m.uid).max().unwrap_or(0) + 1;
                                out.extend(format!(
                                    "* STATUS \"{name}\" (MESSAGES {} UIDNEXT {next} UIDVALIDITY {} HIGHESTMODSEQ {})\r\n{tag} OK status\r\n",
                                    f.messages.len(), f.validity, f.modseq
                                ).bytes());
                            }
                            None => out.extend(format!("{tag} NO no such mailbox\r\n").bytes()),
                        }
                    } else if upper.contains(" EXAMINE ") || upper.contains(" SELECT ") {
                        let name = unq(line.split_whitespace().nth(2).unwrap_or(""));
                        let s = state.lock().unwrap();
                        s.selects.fetch_add(1, Ordering::Relaxed);
                        match s.folders.get(&name) {
                            Some(f) => {
                                selected = Some(name.clone());
                                let next = f.messages.iter().map(|m| m.uid).max().unwrap_or(0) + 1;
                                out.extend(format!(
                                    "* {} EXISTS\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {next}] ok\r\n* OK [HIGHESTMODSEQ {}] ok\r\n{tag} OK [READ-ONLY] done\r\n",
                                    f.messages.len(), f.validity, f.modseq
                                ).bytes());
                            }
                            None => out.extend(format!("{tag} NO nope\r\n").bytes()),
                        }
                    } else if upper.contains("FETCH") {
                        let s = state.lock().unwrap();
                        s.fetches.fetch_add(1, Ordering::Relaxed);
                        let f = selected.as_ref().and_then(|n| s.folders.get(n));
                        if let Some(f) = f {
                            let flags_only = upper.contains("CHANGEDSINCE");
                            let spec = line.split_whitespace().nth(3).unwrap_or("1:*").to_string();
                            let wanted: Vec<&Msg> = if upper.contains("UID FETCH") {
                                if let Some((a, b)) = spec.split_once(':') {
                                    let a: u32 = a.parse().unwrap_or(1);
                                    let max = f.messages.iter().map(|m| m.uid).max().unwrap_or(0);
                                    let b: u32 = if b == "*" {
                                        max
                                    } else {
                                        b.parse().unwrap_or(max)
                                    };
                                    if a > max && b >= max {
                                        // RFC: a range past the end clamps to the last message.
                                        f.messages.iter().rev().take(1).collect()
                                    } else {
                                        f.messages
                                            .iter()
                                            .filter(|m| m.uid >= a && m.uid <= b)
                                            .collect()
                                    }
                                } else {
                                    f.messages.iter().collect()
                                }
                            } else {
                                // sequence window a:b
                                let (a, b) = spec.split_once(':').unwrap_or(("1", "*"));
                                let a: usize = a.parse().unwrap_or(1);
                                let b: usize = if b == "*" {
                                    f.messages.len()
                                } else {
                                    b.parse().unwrap_or(1)
                                };
                                f.messages
                                    .iter()
                                    .skip(a.saturating_sub(1))
                                    .take(b.saturating_sub(a) + 1)
                                    .collect()
                            };
                            for (i, m) in wanted.iter().enumerate() {
                                if flags_only {
                                    out.extend(
                                        format!(
                                            "* {} FETCH (UID {} FLAGS ({}) MODSEQ ({}))\r\n",
                                            i + 1,
                                            m.uid,
                                            m.flags,
                                            f.modseq
                                        )
                                        .bytes(),
                                    );
                                } else {
                                    out.extend(format!(
                                        "* {} FETCH (UID {} FLAGS ({}) BODY[] {{{}}}\r\n{})\r\n",
                                        i + 1, m.uid, m.flags, m.raw.len(), m.raw
                                    ).bytes());
                                }
                            }
                        }
                        out.extend(format!("{tag} OK fetched\r\n").bytes());
                    } else if upper.contains(" LOGOUT") {
                        out.extend(format!("* BYE\r\n{tag} OK bye\r\n").bytes());
                        let _ = tx.write_all(&out).await;
                        return;
                    } else {
                        out.extend(format!("{tag} OK noop\r\n").bytes());
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

fn folders(state: &Arc<Mutex<ServerState>>) -> std::sync::MutexGuard<'_, ServerState> {
    state.lock().unwrap()
}

#[tokio::test]
async fn a_quiet_cycle_is_status_lines_on_one_connection() {
    let state = Arc::new(Mutex::new(ServerState {
        folders: [
            (
                "INBOX".to_string(),
                Folder {
                    validity: 1,
                    modseq: 10,
                    messages: vec![msg(1, "a"), msg(2, "b")],
                },
            ),
            (
                "Receipts".to_string(),
                Folder {
                    validity: 2,
                    modseq: 5,
                    messages: vec![msg(7, "r")],
                },
            ),
        ]
        .into(),
        logins: AtomicUsize::new(0),
        selects: AtomicUsize::new(0),
        fetches: AtomicUsize::new(0),
    }));
    let port = server(Arc::clone(&state)).await;
    let cfg = ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p".into()),
        security: Security::InsecurePlaintext,
    };
    let mk = |since: &[(u32, Option<u32>, Option<u64>)]| -> Vec<FolderPass> {
        [("INBOX", since[0]), ("Receipts", since[1])]
            .iter()
            .map(|(p, (uid, v, m))| FolderPass {
                path: p.to_string(),
                since_uid: *uid,
                expected_validity: *v,
                since_uidnext: None,
                since_modseq: *m,
                seed_window: 200,
            })
            .collect()
    };

    // First cycle: nothing known — both folders seed.
    let mut got = Vec::new();
    let out = sync_pass(
        &cfg,
        &mk(&[(0, None, None), (0, None, None)]),
        |i, uid, _f, _raw| {
            got.push((i, uid));
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(out[0], PassOutcome::Fetched { fetched: 2, .. }),
        "{out:?}"
    );
    assert!(
        matches!(out[1], PassOutcome::Fetched { fetched: 1, .. }),
        "{out:?}"
    );
    assert_eq!(got, vec![(0, 1), (0, 2), (1, 7)]);
    assert_eq!(
        folders(&state).logins.load(Ordering::Relaxed),
        1,
        "one login for the whole pass"
    );

    // Second cycle with watermarks and baselines: pure STATUS, zero selects,
    // zero fetches, and still exactly one more login.
    let before_sel = folders(&state).selects.load(Ordering::Relaxed);
    let before_fetch = folders(&state).fetches.load(Ordering::Relaxed);
    let out = sync_pass(
        &cfg,
        &mk(&[(2, Some(1), Some(10)), (7, Some(2), Some(5))]),
        |_, _, _, _| {
            panic!("a quiet cycle must fetch nothing");
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(out[0], PassOutcome::Unchanged { total: 2, .. }),
        "{out:?}"
    );
    assert!(
        matches!(out[1], PassOutcome::Unchanged { total: 1, .. }),
        "{out:?}"
    );
    assert_eq!(
        folders(&state).selects.load(Ordering::Relaxed),
        before_sel,
        "no selects"
    );
    assert_eq!(
        folders(&state).fetches.load(Ordering::Relaxed),
        before_fetch,
        "no fetches"
    );
    assert_eq!(folders(&state).logins.load(Ordering::Relaxed), 2);

    // New mail in one folder: only that folder is touched, above the watermark.
    folders(&state)
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .push(msg(3, "c"));
    folders(&state).folders.get_mut("INBOX").unwrap().modseq = 11;
    let mut got = Vec::new();
    let out = sync_pass(
        &cfg,
        &mk(&[(2, Some(1), Some(10)), (7, Some(2), Some(5))]),
        |i, uid, _f, _raw| {
            got.push((i, uid));
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(out[0], PassOutcome::Fetched { fetched: 1, .. }),
        "{out:?}"
    );
    assert!(matches!(out[1], PassOutcome::Unchanged { .. }), "{out:?}");
    assert_eq!(got, vec![(0, 3)], "only the new message, only that folder");

    // Flags moved with no new mail: a CONDSTORE diff, no bodies.
    folders(&state).folders.get_mut("Receipts").unwrap().modseq = 6;
    let out = sync_pass(
        &cfg,
        &mk(&[(3, Some(1), Some(11)), (7, Some(2), Some(5))]),
        |_, _, _, _| {
            panic!("flag reconciliation must not refetch bodies");
        },
    )
    .await
    .unwrap();
    let PassOutcome::Fetched {
        fetched,
        flag_updates,
        ..
    } = &out[1]
    else {
        panic!("{out:?}");
    };
    assert_eq!(*fetched, 0);
    assert_eq!(flag_updates.len(), 1, "{flag_updates:?}");

    // A renumbered folder is reported, never fetched.
    folders(&state).folders.get_mut("INBOX").unwrap().validity = 99;
    let out = sync_pass(
        &cfg,
        &mk(&[(3, Some(1), Some(11)), (7, Some(2), Some(6))]),
        |_, _, _, _| {
            panic!("a validity change must fetch nothing");
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(out[0], PassOutcome::ValidityChanged { now: Some(99) }),
        "{out:?}"
    );
}

#[tokio::test]
async fn backfill_walks_history_in_strides_and_knows_when_it_is_done() {
    // A folder holding uids 1..=10; the "seed" already took 8..=10 locally.
    let state = Arc::new(Mutex::new(ServerState {
        folders: [(
            "INBOX".to_string(),
            Folder {
                validity: 1,
                modseq: 1,
                messages: (1..=10).map(|u| msg(u, &format!("m{u}"))).collect(),
            },
        )]
        .into(),
        logins: AtomicUsize::new(0),
        selects: AtomicUsize::new(0),
        fetches: AtomicUsize::new(0),
    }));
    let port = server(Arc::clone(&state)).await;
    let cfg = ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p".into()),
        security: Security::InsecurePlaintext,
    };

    // Stride one: ceiling 8, chunk 3 → asks 5:7.
    let mut got = Vec::new();
    let n = petrel_providers::imap::fetch_uid_range_each(&cfg, "INBOX", 5, 7, |uid, _f, _raw| {
        got.push(uid);
    })
    .await
    .expect("stride");
    assert_eq!(n, 3);
    assert_eq!(got, vec![5, 6, 7]);

    // Stride over a stretch history has emptied: nothing, and not an error.
    state
        .lock()
        .unwrap()
        .folders
        .get_mut("INBOX")
        .unwrap()
        .messages
        .retain(|m| m.uid > 4);
    let n = petrel_providers::imap::fetch_uid_range_each(&cfg, "INBOX", 2, 4, |_, _, _| {
        panic!("these numbers are spent");
    })
    .await
    .expect("empty stride");
    assert_eq!(n, 0, "an expunged stretch is silence, not failure");

    // An inverted range never even connects a fetch.
    let n = petrel_providers::imap::fetch_uid_range_each(&cfg, "INBOX", 5, 4, |_, _, _| {
        panic!("no range, no fetch")
    })
    .await
    .expect("no-op");
    assert_eq!(n, 0);
}
