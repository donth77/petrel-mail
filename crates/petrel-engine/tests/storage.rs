//! The Storage pane's figures.
//!
//! "Mail" is read from the `blobs` ledger rather than by walking the blob
//! directory. That is only honest if the ledger agrees with the disk, so the
//! test writes real blobs and compares the report against what is there.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn bytes_on_disk(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let meta = entry.metadata().unwrap();
        total += if meta.is_dir() {
            bytes_on_disk(&entry.path())
        } else {
            meta.len()
        };
    }
    total
}

#[test]
fn mail_bytes_match_what_is_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let blob_dir = dir.path().join("blobs");
    let blobs = BlobStore::open(&blob_dir).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();

    for i in 0..5 {
        // Bodies long enough that compression changes their size, so a ledger
        // recording the uncompressed length would be caught.
        let raw = format!(
            "From: Sam <sam@example.com>\r\nTo: me@example.com\r\nSubject: m{i}\r\n\r\n{}",
            "the quick brown fox ".repeat(200 + i)
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), None, raw.as_bytes())
            .unwrap();
    }

    let report = store.storage_report(&dir.path().join("petrel.db")).unwrap();
    assert_eq!(report.messages, 5);
    assert!(report.blob_bytes > 0);
    assert_eq!(report.blob_bytes, bytes_on_disk(&blob_dir));
}

#[test]
fn an_empty_store_reports_zero_rather_than_failing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let report = store.storage_report(&dir.path().join("petrel.db")).unwrap();
    assert_eq!(report.messages, 0);
    assert_eq!(report.blob_bytes, 0);
    assert_eq!(report.database_bytes, 0);
}

#[test]
fn each_account_reports_its_own_share() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let a = store.ensure_test_account().unwrap();
    let b = store
        .add_account(
            "imap",
            "other@example.net",
            "Other",
            &petrel_engine::store::AccountServers {
                imap_host: "imap.example.net".into(),
                imap_port: 993,
                smtp_host: "smtp.example.net".into(),
                smtp_port: 465,
                username: "other@example.net".into(),
                provider: String::new(),
            },
        )
        .unwrap();
    let blob_dir = dir.path().join("blobs");
    let blobs = BlobStore::open(&blob_dir).unwrap();
    let inbox_a = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let inbox_b = store.ensure_folder(b, "inbox", "INBOX").unwrap();

    let raw = |id: &str, body: &str| {
        format!(
            "From: Sam <sam@example.com>\r\nTo: me@example.com\r\nMessage-ID: <{id}>\r\nSubject: s\r\n\r\n{}",
            body.repeat(300)
        )
    };
    // Two of A's own, one of B's own, and one both were sent.
    for (acct, folder, id, body) in [
        (a, inbox_a, "a1@x", "alpha "),
        (a, inbox_a, "a2@x", "beta "),
        (b, inbox_b, "b1@x", "gamma "),
        (a, inbox_a, "both@x", "delta "),
        (b, inbox_b, "both@x", "delta "),
    ] {
        store
            .ingest_raw(&blobs, acct, Some(folder), None, raw(id, body).as_bytes())
            .unwrap();
    }

    let report = store.storage_report(&dir.path().join("petrel.db")).unwrap();
    assert_eq!(report.messages, 5);
    // The shared message is one blob on disk...
    assert_eq!(report.blob_bytes, bytes_on_disk(&blob_dir));

    let ids: Vec<i64> = report.accounts.iter().map(|s| s.account_id).collect();
    assert_eq!(ids, vec![a, b]);
    let [sa, sb] = [&report.accounts[0], &report.accounts[1]];
    assert_eq!((sa.messages, sb.messages), (3, 2));
    assert!(sa.blob_bytes > sb.blob_bytes);
    // ...and counts for each of the accounts that hold it, so the shares sum
    // to more than the total by exactly its size.
    let shared = sa.blob_bytes + sb.blob_bytes - report.blob_bytes;
    assert!(
        shared > 0,
        "shared message was not counted for both accounts"
    );
    assert!(
        shared < sb.blob_bytes,
        "B's share should be more than the shared message alone"
    );
}
