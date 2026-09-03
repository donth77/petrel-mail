//! A message that lands between STATUS and EXAMINE.
//!
//! STATUS says "no new mail, flags moved" with UIDNEXT 3. Before EXAMINE
//! answers, UID 3 arrives, so EXAMINE reports UIDNEXT 4. The pass fetches
//! nothing (STATUS said not to) and must not record 4 as the watermark: the
//! next pass would start above the message and it would never be fetched.
//! IDLE wakes a sync precisely while mail is arriving, so the window is real.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, FolderPass, ImapConfig, PassOutcome, Security, sync_pass,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

struct Server {
    uids: Vec<u32>,
    modseq: u64,
    /// Appended to the mailbox the moment EXAMINE is answered.
    arrives_on_examine: Option<u32>,
}

async fn serve(state: Arc<Mutex<Server>>) -> u16 {
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
                let _ = tx.write_all(b"* OK ready\r\n").await;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                    let up = line.to_ascii_uppercase();
                    let mut out = String::new();
                    if up.contains(" LOGIN ") {
                        out += &format!("{tag} OK in\r\n");
                    } else if up.contains(" CAPABILITY") {
                        out += &format!("* CAPABILITY IMAP4rev1 CONDSTORE\r\n{tag} OK\r\n");
                    } else if up.contains(" STATUS ") {
                        let s = state.lock().unwrap();
                        let next = s.uids.iter().max().unwrap_or(&0) + 1;
                        out += &format!(
                            "* STATUS \"INBOX\" (MESSAGES {} UIDNEXT {next} UIDVALIDITY 1 HIGHESTMODSEQ {})\r\n{tag} OK\r\n",
                            s.uids.len(),
                            s.modseq
                        );
                    } else if up.contains(" EXAMINE ") {
                        let mut s = state.lock().unwrap();
                        if let Some(u) = s.arrives_on_examine.take() {
                            s.uids.push(u);
                            s.modseq += 1;
                        }
                        let next = s.uids.iter().max().unwrap_or(&0) + 1;
                        out += &format!(
                            "* {} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT {next}] ok\r\n* OK [HIGHESTMODSEQ {}] ok\r\n{tag} OK [READ-ONLY]\r\n",
                            s.uids.len(),
                            s.modseq
                        );
                    } else if up.contains("FETCH") {
                        let s = state.lock().unwrap();
                        if up.contains("CHANGEDSINCE") {
                            for (i, u) in s.uids.iter().enumerate() {
                                out += &format!(
                                    "* {} FETCH (UID {u} FLAGS (\\Seen) MODSEQ ({}))\r\n",
                                    i + 1,
                                    s.modseq
                                );
                            }
                        } else {
                            let spec = line.split_whitespace().nth(3).unwrap_or("1:*").to_string();
                            let (a, b) = spec.split_once(':').unwrap_or(("1", "*"));
                            let a: u32 = a.parse().unwrap_or(1);
                            let b: u32 = if b == "*" {
                                u32::MAX
                            } else {
                                b.parse().unwrap_or(u32::MAX)
                            };
                            for (i, u) in s
                                .uids
                                .iter()
                                .enumerate()
                                .filter(|(_, u)| **u >= a && **u <= b)
                            {
                                let raw = format!("Subject: m{u}\r\n\r\nbody\r\n");
                                out += &format!(
                                    "* {} FETCH (UID {u} FLAGS (\\Seen) BODY[] {{{}}}\r\n{raw})\r\n",
                                    i + 1,
                                    raw.len()
                                );
                            }
                        }
                        out += &format!("{tag} OK\r\n");
                    } else if up.contains(" LOGOUT") {
                        let _ = tx
                            .write_all(format!("* BYE\r\n{tag} OK\r\n").as_bytes())
                            .await;
                        return;
                    } else {
                        out += &format!("{tag} OK\r\n");
                    }
                    if tx.write_all(out.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

fn pass(since_uid: u32, since_uidnext: u32, since_modseq: u64) -> Vec<FolderPass> {
    vec![FolderPass {
        path: "INBOX".into(),
        since_uid,
        expected_validity: Some(1),
        since_uidnext: Some(since_uidnext),
        since_modseq: Some(since_modseq),
        seed_window: 50,
    }]
}

#[tokio::test]
async fn a_message_landing_between_status_and_examine_is_fetched_next_time() {
    // The store holds UIDs 1 and 2 and last saw UIDNEXT 3, HIGHESTMODSEQ 5.
    let state = Arc::new(Mutex::new(Server {
        uids: vec![1, 2],
        modseq: 6,
        arrives_on_examine: Some(3),
    }));
    let port = serve(Arc::clone(&state)).await;
    let cfg = ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("p"),
        security: Security::InsecurePlaintext,
    };

    // Cycle one: flags moved, UIDNEXT still 3 at STATUS time; UID 3 lands
    // before EXAMINE answers.
    let mut got = Vec::new();
    let out = sync_pass(&cfg, &pass(2, 3, 5), false, |_, uid, _, _| got.push(uid))
        .await
        .unwrap();
    let PassOutcome::Fetched {
        uid_next,
        highest_modseq,
        ..
    } = &out[0]
    else {
        panic!("{out:?}");
    };
    let uid_next = uid_next.expect("a watermark");
    let modseq = highest_modseq.expect("a modseq");

    // Cycle two, with the pass derived exactly as the desktop derives it:
    // since_uid is the higher of the highest held UID and uidnext - 1.
    let since_uid = 2u32.max(uid_next.saturating_sub(1));
    let out2 = sync_pass(
        &cfg,
        &pass(since_uid, uid_next, modseq),
        false,
        |_, uid, _, _| got.push(uid),
    )
    .await
    .unwrap();
    assert!(
        got.contains(&3),
        "UID 3 must reach the store by the second pass; got {got:?}, cycle one recorded uid_next={uid_next}, cycle two {out2:?}"
    );
}
