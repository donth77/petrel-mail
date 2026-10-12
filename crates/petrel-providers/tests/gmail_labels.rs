//! Gmail labels travel the way mailbox names do: modified UTF-7, inside a
//! quoted string whose escapes the parser leaves in.
//!
//! Read raw, a label sweep handed `R&-D`, `&AMQ-rger` and `say \"hi\"` to an
//! engine whose tags are called `R&D`, `Ärger` and `say "hi"`; none matched,
//! and the sweep removed each of those tags from every message it covered —
//! every Japanese, Chinese or Korean tag on a Gmail account. Written raw, a
//! non-ASCII tag went out as 8-bit bytes in a quoted string.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, ImapConfig, Security, store_gmail_labels, sweep_gmail_labels,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type Log = Arc<Mutex<Vec<String>>>;

async fn server() -> (u16, Log) {
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
                    let reply = if upper.contains(" CAPABILITY") {
                        format!("* CAPABILITY IMAP4rev1 X-GM-EXT-1\r\n{tag} OK done\r\n")
                    } else if upper.contains(" SELECT ") || upper.contains(" EXAMINE ") {
                        format!(
                            "* 1 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 2] ok\r\n{tag} OK done\r\n"
                        )
                    } else if upper.contains(" FETCH ") && upper.contains("X-GM-LABELS") {
                        let header = "Message-ID: <m1@x>\r\n\r\n";
                        format!(
                            concat!(
                                "* 1 FETCH (UID 1 X-GM-LABELS (\\Inbox \"R&-D\" \"&AMQ-rger\" ",
                                "\"say \\\"hi\\\"\" Plain \"&ZeVnLIqe-\") ",
                                "BODY[HEADER.FIELDS (MESSAGE-ID)] {{{len}}}\r\n{header})\r\n",
                                "{tag} OK done\r\n"
                            ),
                            len = header.len(),
                            header = header,
                            tag = tag
                        )
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
async fn swept_labels_arrive_as_the_names_people_gave_them() {
    let (port, _log) = server().await;
    let sweep = sweep_gmail_labels(&cfg(port), "[Gmail]/All Mail", 100, None)
        .await
        .expect("sweep");
    assert_eq!(sweep.labels.len(), 1, "{:?}", sweep.labels);
    let (id, labels) = &sweep.labels[0];
    assert_eq!(id, "m1@x");
    assert_eq!(
        labels,
        &vec![
            "\\Inbox".to_string(),
            "R&D".to_string(),
            "Ärger".to_string(),
            "say \"hi\"".to_string(),
            "Plain".to_string(),
            "日本語".to_string(),
        ]
    );
}

#[tokio::test]
async fn a_label_goes_out_in_its_wire_form() {
    let (port, log) = server().await;
    store_gmail_labels(&cfg(port), "[Gmail]/All Mail", 1, Some(1), "Ärger", true)
        .await
        .expect("add");
    store_gmail_labels(&cfg(port), "[Gmail]/All Mail", 1, Some(1), "R&D", false)
        .await
        .expect("remove");
    let wire = log.lock().unwrap().clone();
    let stores: Vec<&String> = wire.iter().filter(|l| l.contains("X-GM-LABELS")).collect();
    assert_eq!(stores.len(), 2, "{wire:?}");
    assert!(
        stores[0].contains("+X-GM-LABELS (\"&AMQ-rger\")"),
        "{}",
        stores[0]
    );
    assert!(
        stores[1].contains("-X-GM-LABELS (\"R&-D\")"),
        "{}",
        stores[1]
    );
    assert!(stores.iter().all(|l| l.is_ascii()));
}
