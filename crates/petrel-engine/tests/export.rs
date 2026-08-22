//! Exporting to mbox — the durability promise.
//!
//! The format matters more than it looks: mbox separates messages on a line
//! beginning "From ", so a body line that happens to start that way splits one
//! message into two when the file is read back. That corruption is invisible
//! at export time and shows up years later in whatever client someone opens
//! the archive with.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, NewMessage, Store};

/// An empty store. Messages are added per test, because "how many were
/// skipped" is one of the things under test and a fixture that quietly
/// contributes blob-less rows makes that number mean nothing.
fn seeded(dir: &std::path::Path) -> (Store, BlobStore, i64) {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let blobs = BlobStore::open(&dir.join("blobs")).unwrap();
    (store, blobs, account)
}

/// Rows with no stored body, the way a header-only sync leaves them.
fn insert_without_blobs(store: &mut Store, account: i64, n: i64) {
    let msgs: Vec<NewMessage> = (0..n)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 1_700_000_000_000 + i * 1000,
            from_addr: "sam@example.com".into(),
            from_display: "Sam".into(),
            to_addr: "me@example.com".into(),
            subject: format!("m{i}"),
            body_text: "body".into(),
        })
        .collect();
    store.insert_messages(&msgs).unwrap();
}

/// Ingests a raw message so the export has real bytes to write.
fn ingest(store: &mut Store, blobs: &BlobStore, account: i64, body: &str) {
    let raw =
        format!("From: Sam <sam@example.com>\r\nTo: me@example.com\r\nSubject: Test\r\n\r\n{body}");
    store
        .ingest_raw(blobs, account, None, None, raw.as_bytes())
        .unwrap();
}

#[test]
fn a_body_line_starting_from_is_escaped_so_it_does_not_split_the_message() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, blobs, account) = seeded(dir.path());
    // The classic case: a quoted line that begins "From ".
    ingest(
        &mut store,
        &blobs,
        account,
        "Quoting you:\r\nFrom here it looks fine.\r\n",
    );

    let out = dir.path().join("out.mbox");
    let (written, _) = store.export_mbox(&blobs, &ListView::Inbox, &out).unwrap();
    assert_eq!(written, 1);

    let text = std::fs::read_to_string(&out).unwrap();
    assert!(
        text.contains(">From here it looks fine."),
        "body line was not escaped:\n{text}"
    );
    // Exactly one separator: the message must not have been split in two.
    let separators = text.lines().filter(|l| l.starts_with("From ")).count();
    assert_eq!(
        separators, 1,
        "message split by an unescaped From line:\n{text}"
    );
}

#[test]
fn every_message_gets_a_separator_readers_can_find() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, blobs, account) = seeded(dir.path());
    for i in 0..3 {
        ingest(&mut store, &blobs, account, &format!("body {i}"));
    }

    let out = dir.path().join("out.mbox");
    let (written, skipped) = store.export_mbox(&blobs, &ListView::Inbox, &out).unwrap();
    assert_eq!(written, 3);
    assert_eq!(skipped, 0);

    let text = std::fs::read_to_string(&out).unwrap();
    let separators = text.lines().filter(|l| l.starts_with("From ")).count();
    assert_eq!(separators, 3);
    // asctime shape, which is what mbox readers parse.
    assert!(
        text.lines().next().unwrap().split_whitespace().count() >= 6,
        "From line is not asctime-shaped: {:?}",
        text.lines().next()
    );
}

#[test]
fn a_missing_blob_is_skipped_rather_than_losing_the_whole_export() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, blobs, account) = seeded(dir.path());
    insert_without_blobs(&mut store, account, 3);
    let out = dir.path().join("out.mbox");
    let (written, skipped) = store.export_mbox(&blobs, &ListView::Inbox, &out).unwrap();
    assert_eq!(written, 0);
    assert_eq!(skipped, 3, "a partial archive beats an error and nothing");
    assert!(out.exists());
}
