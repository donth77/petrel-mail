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

/// The full circle: what export writes, import reads back whole.
mod round_trip {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::{ListView, Store};

    fn fixture(mid: &str, subject: &str, body: &str) -> Vec<u8> {
        format!(
            "From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\n\
             Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
             Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn an_exported_mailbox_imports_whole_and_twice_imports_once() {
        let dir = tempfile::tempdir().unwrap();

        // Store A: three messages, one with a body line the mbox dialect must
        // escape and un-escape.
        let mut a = Store::open(&dir.path().join("a.db")).unwrap();
        let blobs_a = BlobStore::open(&dir.path().join("blobs-a")).unwrap();
        let account_a = a.ensure_test_account().unwrap();
        let inbox_a = a.ensure_folder(account_a, "inbox", "INBOX").unwrap();
        for (i, (mid, subject, body)) in [
            ("r1@x", "first", "plain words"),
            (
                "r2@x",
                "second",
                "quoting a forward:\r\nFrom the desk of Dana Wu",
            ),
            ("r3@x", "third", "closing words"),
        ]
        .iter()
        .enumerate()
        {
            a.ingest_raw(
                &blobs_a,
                account_a,
                Some(inbox_a),
                Some(i as u32 + 1),
                &fixture(mid, subject, body),
            )
            .unwrap();
        }
        let mbox = dir.path().join("take.mbox");
        let (written, skipped) = a.export_mbox(&blobs_a, &ListView::Inbox, &mbox).unwrap();
        assert_eq!((written, skipped), (3, 0));

        // Store B: a different machine, importing the archive.
        let mut b = Store::open(&dir.path().join("b.db")).unwrap();
        let blobs_b = BlobStore::open(&dir.path().join("blobs-b")).unwrap();
        let account_b = b.ensure_test_account().unwrap();
        let imported = b.ensure_named_folder(account_b, "Imported").unwrap();
        b.mark_folder_local(imported).unwrap();

        let messages = petrel_engine::mbox::split(&std::fs::read(&mbox).unwrap());
        assert_eq!(messages.len(), 3);
        let mut new = 0;
        for raw in &messages {
            if b.ingest_raw(&blobs_b, account_b, Some(imported), None, raw)
                .unwrap()
                .was_new
            {
                new += 1;
            }
        }
        assert_eq!(new, 3);

        // Everything arrived as itself — including the escaped line.
        let rows = b
            .list_threads(&ListView::parse(&format!("folder:{imported}")), 0, 50)
            .unwrap();
        assert_eq!(rows.len(), 3, "{rows:?}");
        let hits = b.search_threads("desk of Dana", 10).unwrap();
        assert_eq!(hits.len(), 1, "the escaped body line survived: {hits:?}");

        // Importing the same archive again adds nothing.
        let mut dup = 0;
        for raw in &messages {
            if !b
                .ingest_raw(&blobs_b, account_b, Some(imported), None, raw)
                .unwrap()
                .was_new
            {
                dup += 1;
            }
        }
        assert_eq!(dup, 3);

        // And the local folder outlives a sync survey that has never heard
        // of it — the prune must leave it alone.
        b.sync_folders(account_b, &[("INBOX".into(), Some("inbox".into()))])
            .unwrap();
        assert!(
            b.folders(account_b)
                .unwrap()
                .iter()
                .any(|f| f.id == imported),
            "a local folder is not the survey's to prune"
        );
    }
}
