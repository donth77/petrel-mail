//! Mailbox names are modified UTF-7 on the wire (RFC 3501 §5.1.3).
//!
//! Petrel used the wire spelling as the name: a German Drafts folder showed as
//! `Entw&APw-rfe` in the rail, in the move menu and everywhere else, and a
//! Japanese folder as a line of base64. The other direction was worse than
//! ugly — a CREATE or a SELECT carrying raw UTF-8 is refused by every server
//! that has not been told to accept it, so a folder with an accent in its name
//! could not be made, opened, or moved into.
#![cfg(feature = "insecure-plaintext")]

use std::sync::{Arc, Mutex};

use petrel_providers::imap::{
    Credential, ImapConfig, Security, create_folder, probe, rename_folder,
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
                        format!("* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n")
                    } else if upper.contains(" LIST ") {
                        format!(
                            concat!(
                                "* LIST (\\HasNoChildren) \"/\" INBOX\r\n",
                                "* LIST (\\HasNoChildren \\Drafts) \"/\" \"Entw&APw-rfe\"\r\n",
                                "* LIST (\\HasNoChildren \\Sent) \"/\" \"&kAFP4W4IMH8wojCk-\"\r\n",
                                "* LIST (\\HasNoChildren) \"/\" \"Tom &- Jerry\"\r\n",
                                "* LIST (\\HasNoChildren) \"/\" \"a \\\"quoted\\\" name\"\r\n",
                                "{tag} OK listed\r\n"
                            ),
                            tag = tag
                        )
                    } else if upper.contains(" SELECT ") || upper.contains(" EXAMINE ") {
                        format!(
                            "* 0 EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n* OK [UIDNEXT 1] ok\r\n{tag} OK done\r\n"
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
async fn listed_names_arrive_as_words_and_go_back_as_the_wire_spells_them() {
    let (port, log) = server().await;
    let report = probe(&cfg(port), 0).await.expect("probe");
    let names: Vec<String> = report.folders.iter().map(|f| f.name.clone()).collect();
    assert_eq!(
        names,
        vec![
            "INBOX".to_string(),
            "Entwürfe".to_string(),
            "送信済みアイ".to_string(),
            "Tom & Jerry".to_string(),
            "a \"quoted\" name".to_string(),
        ],
        "decoded, and the quoted-string escapes undone"
    );

    // And back the other way: what goes out is the wire spelling, so the
    // server recognises the folder it is being asked about.
    create_folder(&cfg(port), "Entwürfe/Été 2026")
        .await
        .expect("create");
    rename_folder(&cfg(port), "Tom & Jerry", "下書き")
        .await
        .expect("rename");
    let wire = log.lock().unwrap().clone();
    let create = wire
        .iter()
        .find(|l| l.contains("CREATE"))
        .expect("a CREATE went out");
    assert!(
        create.contains("Entw&APw-rfe/&AMk-t&AOk- 2026"),
        "encoded on the wire: {create}"
    );
    assert!(create.is_ascii(), "and nothing but ASCII: {create}");
    let rename = wire
        .iter()
        .find(|l| l.contains("RENAME"))
        .expect("a RENAME went out");
    assert!(
        rename.contains("Tom &- Jerry") && rename.contains("&Tgtm+DBN-"),
        "both names encoded, the literal ampersand doubled: {rename}"
    );

    // The names the server listed come back out spelled exactly as it sent
    // them, which is what makes a decoded name safe to store and select by.
    let sent = wire
        .iter()
        .find(|l| l.contains("SELECT") || l.contains("EXAMINE"))
        .cloned();
    assert!(sent.is_some(), "the probe selects the inbox: {wire:?}");
    for folder in &names[1..] {
        create_folder(&cfg(port), folder).await.expect("create");
    }
    let wire = log.lock().unwrap().clone();
    for expected in [
        "\"Entw&APw-rfe\"",
        "\"&kAFP4W4IMH8wojCk-\"",
        "\"Tom &- Jerry\"",
        "\"a \\\"quoted\\\" name\"",
    ] {
        assert!(
            wire.iter().any(|l| l.contains(expected)),
            "round trip: nothing on the wire said {expected}\n{wire:?}"
        );
    }
}
