//! M1 — the vertical slice: server → engine → search.
//!
//! Every piece has been proven in isolation (IMAP fetch, MIME parse,
//! transactional index). This asserts they compose: mail that exists only on a
//! server ends up searchable locally, byte-exact, and a second sync of the same
//! mailbox does not duplicate it.
//!
//! Needs the test server (testkit/README.md). Run:
//!   cargo test -p petrel-providers --features insecure-plaintext \
//!     --test sync_vertical -- --ignored --nocapture

#![cfg(feature = "insecure-plaintext")]

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;
use petrel_providers::imap::{Credential, ImapConfig, Security, append_message, fetch_raw};

fn cfg() -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        port: 3143,
        user: "petrel".into(),
        credential: Credential::password("petrelpass"),
        security: Security::InsecurePlaintext,
    }
}

fn message(n: usize, token: &str) -> Vec<u8> {
    format!(
        "From: Vendor {n} <vendor{n}@example.com>\r\n\
         To: petrel@example.com\r\n\
         Subject: Sync vertical {n}\r\n\
         Date: Thu, 20 Aug 2026 1{n}:00:00 +0000\r\n\
         Message-ID: <vertical-{n}-{token}@petrel.test>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         Body {n}: the {token} quote covers quarterly logistics.\r\n"
    )
    .into_bytes()
}

#[tokio::test]
#[ignore = "requires the local IMAP test server"]
async fn mail_on_a_server_becomes_searchable_locally() {
    let cfg = cfg();
    // Unique per run: the mailbox is shared and this test must not depend on
    // being the only thing that ever wrote to it.
    let token = format!("kestrelmark{}", std::process::id());

    for n in 1..=3 {
        append_message(&cfg, "INBOX", None, &message(n, &token))
            .await
            .expect("seed server");
    }

    let fetched = fetch_raw(&cfg, "INBOX", 50).await.expect("fetch raw");
    assert!(!fetched.is_empty(), "server should return messages");

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");

    let mut ingested = 0usize;
    for (uid, raw) in &fetched {
        if store
            .ingest_raw(&blobs, account, None, Some(*uid), raw)
            .is_ok()
        {
            ingested += 1;
        }
    }
    println!("fetched {} message(s), ingested {ingested}", fetched.len());
    assert!(ingested >= 3);

    // The point of the whole exercise: it is findable offline, by its content.
    let hits = store.search(&token, 20).expect("search");
    println!("search {token:?} -> {} hit(s)", hits.len());
    assert_eq!(
        hits.len(),
        3,
        "all three seeded messages should be searchable"
    );

    // Sender and subject search paths work over real parsed headers too.
    assert!(
        !store
            .search("vendor1@example.com", 10)
            .expect("search")
            .is_empty()
    );
    assert!(
        !store
            .search("Sync vertical", 10)
            .expect("search")
            .is_empty()
    );

    // A second sync of the same mailbox must not multiply the mail.
    let before = store.message_count().expect("count");
    for (uid, raw) in &fetched {
        let _ = store.ingest_raw(&blobs, account, None, Some(*uid), raw);
    }
    assert_eq!(
        store.message_count().expect("count"),
        before,
        "re-syncing the same messages must not duplicate them"
    );

    store.fts_integrity_check().expect("index consistent");
}
