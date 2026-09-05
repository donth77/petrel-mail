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

/// Every read the desktop routes through a secondary, on a secondary.
///
/// `query_only` makes a hidden write fail loudly, so the way to know none of
/// these methods writes is to run each of them here. The desktop's own tests
/// run with the readers switched off, so nothing there would catch one.
#[test]
fn every_read_the_desktop_routes_works_on_a_secondary() {
    use petrel_engine::store::{ListView, Sort, SortKey};
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("petrel.db");
    let mut store = Store::open(&path).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let raw = b"From: Sam <sam@example.com>\r\n\
        To: me@example.com\r\n\
        Subject: secondary read\r\n\
        Date: Sat, 5 Sep 2026 14:00:00 +0000\r\n\
        Message-ID: <routed@example.com>\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\r\n\
        hello there \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e\r\n";
    let ingested = store
        .ingest_raw(&blobs, account, Some(inbox), Some(1), raw)
        .expect("ingest");

    let r = Store::open_secondary(&path).expect("secondary");
    let newest_first = Sort {
        key: SortKey::Date,
        ascending: false,
    };
    let rows = r
        .list_threads(&ListView::parse("inbox"), 0, 50, newest_first)
        .expect("list_threads");
    assert_eq!(rows.len(), 1);
    let thread = rows[0].thread_id;
    r.list_threads_after(
        &ListView::parse("inbox"),
        50,
        newest_first,
        rows[0].date_ms,
        thread,
    )
    .expect("list_threads_after");
    for view in [
        "inbox", "starred", "sent", "drafts", "archive", "trash", "spam", "outbox", "snoozed",
        "all",
    ] {
        let view = ListView::parse(view);
        r.list_threads(&view, 0, 50, newest_first)
            .unwrap_or_else(|e| panic!("list_threads {view:?}: {e}"));
        r.conversations_in(&view)
            .unwrap_or_else(|e| panic!("conversations_in {view:?}: {e}"));
    }
    for key in [SortKey::Sender, SortKey::Subject] {
        r.list_threads(
            &ListView::parse("inbox"),
            0,
            50,
            Sort {
                key,
                ascending: true,
            },
        )
        .expect("sorted listing");
    }
    r.thread_by_id(thread).expect("thread_by_id");
    r.thread_detail(thread).expect("thread_detail");
    r.thread_index(thread).expect("thread_index");
    r.thread_message(ingested.message_id)
        .expect("thread_message");
    r.view_counts(&std::collections::HashMap::new())
        .expect("view_counts");
    r.search_threads_sorted("hello", 200, None)
        .expect("search, best match");
    r.search_threads_sorted("hello", 200, Some(newest_first))
        .expect("search, sorted");
    r.search_threads_sorted("日本語", 200, None)
        .expect("search, CJK");
    r.search_threads_sorted("from:sam", 200, None)
        .expect("search, operator");
    r.blob_hash_for(ingested.message_id).expect("blob_hash_for");
    r.settings().expect("settings");
    r.remote_content_allowed(ingested.message_id)
        .expect("remote_content_allowed");
    r.message_sender(ingested.message_id)
        .expect("message_sender");
    r.sender_trusted(account, "sam@example.com")
        .expect("sender_trusted");
    r.has_written_to(account, "sam@example.com")
        .expect("has_written_to");
    r.active_account().expect("active_account");
    r.account_servers(account).expect("account_servers");
    r.message_count_for(account).expect("message_count_for");
    r.retention_mode(account).expect("retention_mode");
    r.thread_of(ingested.message_id).expect("thread_of");
}
