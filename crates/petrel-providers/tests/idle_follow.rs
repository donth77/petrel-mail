//! The watch on the open folder, proven against a scripted server.
//!
//! The claims that matter: one login however many folders it follows; a
//! change of folder is an EXAMINE on the connection already open; a wake
//! names the folder that spoke; and nothing to watch is no connection at all.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use petrel_providers::imap::{Credential, ImapConfig, Security, idle_follow};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};

#[derive(Default)]
struct Seen {
    logins: usize,
    logouts: usize,
    /// Every IDLE the client has started.
    idles: usize,
    examined: Vec<String>,
}

/// A server that answers enough IMAP to be watched, and says `EXISTS` to
/// whichever connection is idling when `poke` is notified.
async fn server() -> (Arc<Mutex<Seen>>, Arc<Notify>, u16) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let poke = Arc::new(Notify::new());
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (s, p) = (Arc::clone(&seen), Arc::clone(&poke));
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let (seen, poke) = (Arc::clone(&s), Arc::clone(&p));
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                let _ = tx.write_all(b"* OK scripted ready\r\n").await;
                let mut line = String::new();
                let mut idling: Option<String> = None;
                loop {
                    // Cleared only once a whole line is handled: a poke that
                    // wins the race leaves a half-read line where it was.
                    let n = if idling.is_some() {
                        tokio::select! {
                            r = reader.read_line(&mut line) => r.unwrap_or(0),
                            _ = poke.notified() => {
                                if tx.write_all(b"* 4 EXISTS\r\n").await.is_err() {
                                    return;
                                }
                                continue;
                            }
                        }
                    } else {
                        reader.read_line(&mut line).await.unwrap_or(0)
                    };
                    if n == 0 {
                        return;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    let upper = line.to_ascii_uppercase();
                    let mut out: Vec<u8> = Vec::new();
                    if let Some(idle_tag) = idling.take() {
                        assert_eq!(upper.trim(), "DONE", "a session in IDLE takes nothing else");
                        out.extend(format!("{idle_tag} OK idle done\r\n").bytes());
                    } else if upper.contains(" LOGIN ") {
                        seen.lock().unwrap().logins += 1;
                        out.extend(format!("{tag} OK in\r\n").bytes());
                    } else if upper.contains(" CAPABILITY") {
                        out.extend(b"* CAPABILITY IMAP4rev1 IDLE\r\n".iter());
                        out.extend(format!("{tag} OK done\r\n").bytes());
                    } else if upper.contains(" EXAMINE ") || upper.contains(" SELECT ") {
                        let name = line.split_whitespace().nth(2).unwrap_or("");
                        seen.lock()
                            .unwrap()
                            .examined
                            .push(name.trim_matches('"').to_string());
                        out.extend(
                            format!(
                                "* 3 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 4] ok\r\n{tag} OK [READ-ONLY] done\r\n"
                            )
                            .bytes(),
                        );
                    } else if upper.contains(" IDLE") {
                        seen.lock().unwrap().idles += 1;
                        idling = Some(tag);
                        out.extend(b"+ idling\r\n".iter());
                    } else if upper.contains(" LOGOUT") {
                        seen.lock().unwrap().logouts += 1;
                        out.extend(format!("* BYE\r\n{tag} OK bye\r\n").bytes());
                        let _ = tx.write_all(&out).await;
                        return;
                    } else {
                        out.extend(format!("{tag} OK noop\r\n").bytes());
                    }
                    line.clear();
                    if tx.write_all(&out).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (seen, poke, port)
}

fn plain(port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p"),
        security: Security::InsecurePlaintext,
    }
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn one_connection_follows_the_open_folder() {
    let (seen, poke, port) = server().await;
    let cfg = plain(port);
    let (open, mut follow) = watch::channel(Some("Receipts".to_string()));
    let woke = Arc::new(Mutex::new(Vec::<String>::new()));
    let w = Arc::clone(&woke);
    let task = tokio::spawn(async move {
        idle_follow(&cfg, &mut follow, Duration::from_secs(60), |f| {
            w.lock().unwrap().push(f.to_string())
        })
        .await
    });

    until("the first IDLE", || seen.lock().unwrap().idles == 1).await;
    poke.notify_one();
    until("a wake", || woke.lock().unwrap().len() == 1).await;
    assert_eq!(woke.lock().unwrap()[0], "Receipts");
    until("IDLE again after the wake", || {
        seen.lock().unwrap().idles == 2
    })
    .await;

    // The person clicks another folder. Same socket: DONE, EXAMINE, IDLE.
    open.send_replace(Some("Projects".to_string()));
    until("the new folder watched", || seen.lock().unwrap().idles == 3).await;
    assert_eq!(seen.lock().unwrap().examined, vec!["Receipts", "Projects"]);
    poke.notify_one();
    until("a second wake", || woke.lock().unwrap().len() == 2).await;
    assert_eq!(
        woke.lock().unwrap()[1],
        "Projects",
        "the wake names who spoke"
    );
    assert_eq!(seen.lock().unwrap().logins, 1, "one login for both folders");

    // Back to the inbox, which has its own watch: nothing to follow here.
    open.send_replace(None);
    let ended = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the watch ends")
        .unwrap();
    assert!(ended.is_ok(), "{ended:?}");
    assert_eq!(seen.lock().unwrap().logouts, 1, "logged out, not dropped");
}

#[tokio::test]
async fn the_watch_ends_when_nobody_can_aim_it() {
    let (seen, _poke, port) = server().await;
    let cfg = plain(port);
    let (open, mut follow) = watch::channel(Some("Receipts".to_string()));
    let task = tokio::spawn(async move {
        idle_follow(&cfg, &mut follow, Duration::from_secs(60), |_| {}).await
    });
    until("the first IDLE", || seen.lock().unwrap().idles == 1).await;
    // A closed channel must end the watch, not spin on a change that will
    // never come.
    drop(open);
    let ended = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the watch ends")
        .unwrap();
    assert!(ended.is_ok(), "{ended:?}");
    assert_eq!(seen.lock().unwrap().idles, 1);
    assert_eq!(seen.lock().unwrap().logouts, 1);
}

#[tokio::test]
async fn nothing_to_watch_opens_no_connection() {
    let (seen, _poke, port) = server().await;
    let (_open, mut follow) = watch::channel(None::<String>);
    idle_follow(&plain(port), &mut follow, Duration::from_secs(60), |_| {
        panic!("nothing is watched")
    })
    .await
    .unwrap();
    assert_eq!(seen.lock().unwrap().logins, 0);
}
