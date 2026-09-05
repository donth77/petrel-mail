//! A secondary connection reads committed rows while the writer is busy.
//!
//! The desktop used to wrap the only rusqlite connection in one mutex, so a
//! SELECT for the reading pane waited behind ingest and recounts. WAL already
//! allows the concurrent read; this is the connection that uses it.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn fixture(message_id: &str) -> Vec<u8> {
    format!(
        "From: Sam <sam@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: secondary read\r\n\
         Date: Sat, 5 Sep 2026 14:00:00 +0000\r\n\
         Message-ID: <{message_id}>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         body\r\n"
    )
    .into_bytes()
}

#[test]
fn a_secondary_reads_while_the_writer_holds_a_transaction() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("petrel.db");
    let mut store = Store::open(&path).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let raw = fixture("wal-reader@example.com");
    let ingested = store
        .ingest_raw(&blobs, account, None, Some(1), &raw)
        .expect("ingest");

    let reader = Store::open_secondary(&path).expect("secondary");
    let hash = reader
        .blob_hash_for(ingested.message_id)
        .expect("hash")
        .expect("stored");
    assert_eq!(hash, ingested.blob_hash);

    let thread_id = reader
        .thread_of(ingested.message_id)
        .expect("thread")
        .expect("threaded");
    let cards = reader.thread_index(thread_id).expect("index");
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, ingested.message_id);

    store
        .with_uncommitted_write(|| {
            let again = reader
                .blob_hash_for(ingested.message_id)
                .expect("hash under write lock")
                .expect("still stored");
            assert_eq!(again, ingested.blob_hash);
            let cards = reader
                .thread_index(thread_id)
                .expect("index under write lock");
            assert_eq!(cards[0].id, ingested.message_id);
        })
        .expect("immediate tx");
}

#[test]
fn three_secondaries_read_at_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("petrel.db");
    let mut store = Store::open(&path).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            None,
            Some(1),
            &fixture("three-readers@example.com"),
        )
        .expect("ingest");

    let a = Store::open_secondary(&path).expect("a");
    let b = Store::open_secondary(&path).expect("b");
    let open = Store::open_secondary(&path).expect("open");

    store
        .with_uncommitted_write(|| {
            let ha = a
                .blob_hash_for(ingested.message_id)
                .expect("a")
                .expect("stored");
            let hb = b
                .thread_index(
                    a.thread_of(ingested.message_id)
                        .expect("thread")
                        .expect("id"),
                )
                .expect("index");
            let ho = open
                .blob_hash_for(ingested.message_id)
                .expect("open")
                .expect("stored");
            assert_eq!(ha, ingested.blob_hash);
            assert_eq!(ho, ingested.blob_hash);
            assert_eq!(hb.len(), 1);
        })
        .expect("immediate tx");
}

#[test]
fn a_secondary_cannot_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("petrel.db");
    let store = Store::open(&path).expect("store");
    let _ = store.ensure_test_account().expect("account");
    let reader = Store::open_secondary(&path).expect("secondary");
    let err = reader
        .set_setting("theme", "dark")
        .expect_err("query_only must refuse writes");
    let msg = err.to_string();
    assert!(
        msg.contains("readonly") || msg.contains("read-only") || msg.contains("query_only"),
        "refused write, not a different failure: {msg}"
    );
}
