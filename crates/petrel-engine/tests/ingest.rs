//! M1 — ingestion: raw RFC822 in, searchable message out.
//!
//! This is the seam where the three M0 pieces meet: bytes from a server, the
//! MIME parser, and the store's transactional index. The properties that matter
//! are not "it parsed" but: **a resync cannot duplicate mail**, **the raw bytes
//! survive verbatim**, and **the index describes what was actually stored**.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn fixture(message_id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{message_id}>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         {body}\r\n"
    )
    .into_bytes()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    (dir, store, blobs, account)
}

#[test]
fn ingested_mail_is_searchable_and_byte_exact() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = fixture(
        "vendor-1@example.com",
        "Q3 vendor contracts",
        "Let's lock pricing before Friday. The heliotrope quote is attached in spirit.",
    );

    let out = store
        .ingest_raw(&blobs, account, None, Some(7), &raw)
        .expect("ingest");
    assert!(out.was_new);

    // Searchable by body, subject, and sender — through the real index.
    for query in ["heliotrope", "vendor contracts", "dana@example.com"] {
        let hits = store.search(query, 10).expect("search");
        assert!(
            hits.iter().any(|h| h.message_id == out.message_id),
            "query {query:?} should find the ingested message"
        );
    }

    // The original bytes come back untouched: the parse is a view, not the truth.
    let stored = blobs.read(&out.blob_hash).expect("read blob");
    assert_eq!(stored, raw, "raw message must round-trip byte-for-byte");

    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn resync_of_the_same_message_updates_instead_of_duplicating() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = fixture("stable-id@example.com", "Original subject", "first body");

    let first = store
        .ingest_raw(&blobs, account, None, Some(1), &raw)
        .expect("ingest");
    assert!(first.was_new);

    // A resync re-fetches the identical bytes — the common case, every sync.
    let again = store
        .ingest_raw(&blobs, account, None, Some(1), &raw)
        .expect("re-ingest");
    assert!(
        !again.was_new,
        "same Message-ID must not create a second row"
    );
    assert_eq!(again.message_id, first.message_id);
    assert_eq!(store.message_count().expect("count"), 1);

    // And a server-side edit (same ID, changed content) refreshes the index
    // rather than leaving a stale copy searchable.
    let edited = fixture(
        "stable-id@example.com",
        "Amended subject",
        "second body zephyr",
    );
    let third = store
        .ingest_raw(&blobs, account, None, Some(1), &edited)
        .expect("edit");
    assert_eq!(third.message_id, first.message_id);
    assert_eq!(store.message_count().expect("count"), 1);
    assert_eq!(store.search("zephyr", 10).expect("search").len(), 1);
    assert!(
        store.search("first body", 10).expect("search").is_empty(),
        "superseded content must not linger in the index"
    );
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn message_without_message_id_still_deduplicates() {
    let (_dir, mut store, blobs, account) = setup();
    // Message-ID is a SHOULD, not a MUST; plenty of real mail omits it.
    let raw = b"From: nobody@example.com\r\nSubject: no id here\r\n\r\nbody text\r\n";

    let a = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("first");
    let b = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("second");
    assert_eq!(
        a.message_id, b.message_id,
        "content hash must stand in for a missing Message-ID"
    );
    assert_eq!(store.message_count().expect("count"), 1);
}

#[test]
fn html_only_mail_is_findable_by_its_visible_text() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = b"From: news@example.com\r\n\
Subject: newsletter\r\n\
Message-ID: <html-1@example.com>\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n\
<html><body><p>The <b>quarterly</b> figures are ready.</p>\
<script>var tracking='beacon';</script></body></html>\r\n";

    let out = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("ingest");
    let hits = store.search("quarterly figures", 10).expect("search");
    assert!(hits.iter().any(|h| h.message_id == out.message_id));
    // Script contents must not be searchable — they are not text the user saw.
    assert!(store.search("beacon", 10).expect("search").is_empty());
}

#[test]
fn attachments_are_recorded_and_searchable_by_filename() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = b"From: a@example.com\r\n\
Subject: contract\r\n\
Message-ID: <att-1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=B\r\n\r\n\
--B\r\n\
Content-Type: text/plain\r\n\r\n\
Signed copy attached.\r\n\
--B\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"vendor-agreement.pdf\"\r\n\r\n\
%PDF-1.4\r\n\
--B--\r\n";

    let out = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("ingest");
    // Users search for the filename they remember, not the body.
    let hits = store.search("vendor-agreement", 10).expect("search");
    assert!(
        hits.iter().any(|h| h.message_id == out.message_id),
        "attachment filenames belong in the index"
    );
}

#[test]
fn unparseable_input_does_not_poison_the_store() {
    let (_dir, mut store, blobs, account) = setup();
    let good = fixture("ok@example.com", "fine", "readable body");
    store
        .ingest_raw(&blobs, account, None, None, &good)
        .expect("good");

    // Garbage may fail to ingest, but it must leave the store usable and
    // consistent — one bad message can't take the mailbox down with it.
    let _ = store.ingest_raw(&blobs, account, None, None, &[0xFF; 512]);
    let _ = store.ingest_raw(&blobs, account, None, None, b"");

    assert_eq!(store.search("readable", 10).expect("search").len(), 1);
    store.fts_integrity_check().expect("index still consistent");
}

#[test]
fn mail_survives_closing_and_reopening_the_store() {
    // Persistence is the difference between a cache and a mail client. Prove
    // the store reopens with its mail, its index, and its blobs intact —
    // including full-text search, which is derived state and therefore the
    // thing most likely to be silently missing after a restart.
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("petrel.db");
    let blob_dir = dir.path().join("blobs");
    let raw = fixture(
        "persist@example.com",
        "Kept across restarts",
        "cormorant ledger entry",
    );

    let (id, hash) = {
        let mut store = Store::open(&db).expect("store");
        let blobs = BlobStore::open(&blob_dir).expect("blobs");
        let account = store.ensure_test_account().expect("account");
        let out = store
            .ingest_raw(&blobs, account, None, Some(1), &raw)
            .expect("ingest");
        (out.message_id, out.blob_hash)
    }; // both handles dropped: the process is effectively gone

    let store = Store::open(&db).expect("reopen store");
    let blobs = BlobStore::open(&blob_dir).expect("reopen blobs");

    assert_eq!(store.message_count().expect("count"), 1);
    let hits = store.search("cormorant", 10).expect("search");
    assert_eq!(
        hits.len(),
        1,
        "the index must survive a restart, not just the rows"
    );
    assert_eq!(hits[0].message_id, id);
    assert_eq!(
        blobs.read(&hash).expect("blob"),
        raw,
        "bytes survive verbatim"
    );
    assert!(store.first_account().expect("accounts").is_some());
    store
        .fts_integrity_check()
        .expect("index consistent after reopen");
}
