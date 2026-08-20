//! Spike S3 — IMAP slice.
//!
//! Two targets, both `#[ignore]`d so ordinary `cargo test` stays offline.
//!
//! `local::imap_slice_end_to_end` runs against a throwaway container over
//! plaintext and writes freely. Needs the `insecure-plaintext` feature and the
//! server from testkit/README.md:
//!
//! ```text
//! cargo test -p petrel-providers --features insecure-plaintext \
//!   --test imap_slice -- --ignored --nocapture
//! ```
//!
//! `live_provider_probe` runs against a real server over TLS, read-only, with
//! credentials from the environment (see `.env.example`). Its output is
//! redacted — no subjects, no addresses — because it touches a real mailbox:
//!
//! ```text
//! set -a && . ./.env.local && set +a
//! cargo test -p petrel-providers --test imap_slice live_provider -- --ignored --nocapture
//! ```

use petrel_providers::imap::{ImapConfig, Security};

fn env_config(security: Security) -> Option<ImapConfig> {
    Some(ImapConfig {
        host: std::env::var("PETREL_IMAP_HOST").ok()?,
        port: std::env::var("PETREL_IMAP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(993),
        user: std::env::var("PETREL_IMAP_USER").ok()?,
        pass: std::env::var("PETREL_IMAP_PASS").ok()?,
        security,
    })
}

/// Live mailboxes hold real correspondence. Report shape, never content.
fn redact(s: &str) -> String {
    let chars = s.chars().count();
    if chars == 0 {
        "«empty»".to_string()
    } else {
        format!("«{chars} chars»")
    }
}

#[tokio::test]
#[ignore = "requires a real account: set PETREL_IMAP_* (see .env.example)"]
async fn live_provider_probe() {
    // This test needs an external account that only a human can provision, so
    // absent credentials are "not applicable", not "broken" — a bulk
    // `--ignored` run (as CI does) must not fail because of it. The skip is
    // printed loudly: a quiet skip would be indistinguishable from a pass.
    let Some(cfg) = env_config(Security::Tls) else {
        println!(
            "SKIPPED live_provider_probe: no credentials. Set PETREL_IMAP_HOST / \
             PETREL_IMAP_USER / PETREL_IMAP_PASS (copy .env.example to .env.local, \
             then `set -a && . ./.env.local && set +a`) to run it."
        );
        return;
    };

    // Read-only by construction: probe() never appends, moves, or sets flags.
    let report = petrel_providers::imap::probe(&cfg, 5)
        .await
        .expect("probe live server");

    println!("--- live provider probe ({}) ---", cfg.host);
    println!(
        "capabilities: {}",
        report.greeting_capabilities.raw.join(" ")
    );
    let c = &report.greeting_capabilities;
    println!(
        "flags: idle={} condstore={} qresync={} objectid={} compress={} uidplus={} move={} special-use={}",
        c.idle, c.condstore, c.qresync, c.objectid, c.compress, c.uidplus, c.move_, c.special_use
    );
    println!("chosen sync strategy: {:?}", report.strategy);
    println!("folders: {}", report.folders.len());
    for f in report.folders.iter().take(15) {
        println!("  {:?} {}", f.attributes, f.name);
    }
    println!(
        "INBOX: exists={} uidvalidity={:?} uidnext={:?} highestmodseq={:?}",
        report.inbox.exists,
        report.inbox.uid_validity,
        report.inbox.uid_next,
        report.inbox.highest_modseq
    );
    for h in &report.headers {
        // Redacted on purpose — this is the account owner's real mail.
        println!(
            "  uid={:?} seen={} size={:?} from={} subject={}",
            h.uid,
            h.seen,
            h.size,
            redact(&h.from),
            redact(&h.subject)
        );
    }

    assert!(
        !report.folders.is_empty(),
        "a real account must expose folders"
    );
    assert!(
        report
            .greeting_capabilities
            .raw
            .iter()
            .any(|c| c.eq_ignore_ascii_case("IMAP4REV1")),
        "server should advertise IMAP4rev1"
    );
}

#[cfg(feature = "insecure-plaintext")]
mod local {
    use super::env_config;
    use petrel_providers::imap::{ImapConfig, Security, SyncStrategy, append_message, probe};

    fn cfg() -> ImapConfig {
        env_config(Security::InsecurePlaintext).unwrap_or(ImapConfig {
            host: "127.0.0.1".into(),
            port: 3143,
            user: "petrel".into(),
            pass: "petrelpass".into(),
            security: Security::InsecurePlaintext,
        })
    }

    fn synthetic_message(n: usize) -> Vec<u8> {
        format!(
            "From: Sender {n} <sender{n}@example.com>\r\n\
             To: petrel@example.com\r\n\
             Subject: Petrel slice test {n}\r\n\
             Date: Wed, 20 Aug 2026 0{n}:00:00 +0000\r\n\
             Message-ID: <slice-{n}@petrel.test>\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n\
             Body of synthetic message {n}. quarterly report attached in spirit only.\r\n"
        )
        .into_bytes()
    }

    #[tokio::test]
    #[ignore = "requires the local IMAP test server"]
    async fn imap_slice_end_to_end() {
        let cfg = cfg();

        for n in 1..=3 {
            append_message(&cfg, "INBOX", &synthetic_message(n))
                .await
                .expect("append synthetic message");
        }

        let report = probe(&cfg, 10).await.expect("probe server");

        println!("--- S3: IMAP slice ---");
        println!(
            "capabilities: {}",
            report.greeting_capabilities.raw.join(" ")
        );
        let c = &report.greeting_capabilities;
        println!(
            "flags: idle={} condstore={} qresync={} objectid={} compress={} uidplus={} move={} special-use={}",
            c.idle,
            c.condstore,
            c.qresync,
            c.objectid,
            c.compress,
            c.uidplus,
            c.move_,
            c.special_use
        );
        println!("chosen sync strategy: {:?}", report.strategy);
        println!(
            "folders ({}): {}",
            report.folders.len(),
            report
                .folders
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "INBOX: exists={} uidvalidity={:?} uidnext={:?} highestmodseq={:?}",
            report.inbox.exists,
            report.inbox.uid_validity,
            report.inbox.uid_next,
            report.inbox.highest_modseq
        );
        for h in &report.headers {
            println!(
                "  uid={:?} seen={} size={:?} from={} subject={}",
                h.uid, h.seen, h.size, h.from, h.subject
            );
        }

        // The slice must actually round-trip mail, not merely connect.
        assert!(
            report.inbox.exists >= 3,
            "appended messages should be present"
        );
        assert!(
            !report.folders.is_empty(),
            "server must report at least INBOX"
        );
        assert!(
            report
                .headers
                .iter()
                .any(|h| h.subject.contains("Petrel slice test")),
            "fetched headers should include our appended messages"
        );
        assert!(
            report.headers.iter().all(|h| h.uid.is_some()),
            "UID FETCH must return UIDs — the sync engine keys on them"
        );

        // Strategy must follow the advertised capabilities, not wishful thinking.
        let expected = if c.qresync {
            SyncStrategy::Qresync
        } else if c.condstore {
            SyncStrategy::Condstore
        } else {
            SyncStrategy::FullReconcile
        };
        assert_eq!(report.strategy, expected);
    }
}
