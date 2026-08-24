//! User folders: a location you made, listed, renamed, and removed —
//! without the mail that passed through it ever being destroyed.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Store};

fn fixture(mid: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\n\
         Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nbody {subject}\r\n"
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
fn a_user_folder_view_lists_exactly_what_is_placed_there() {
    let (_dir, mut store, blobs, account) = setup();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_named_folder(account, "Receipts").unwrap();

    store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(1),
            &fixture("a@x", "in the inbox"),
        )
        .unwrap();
    store
        .ingest_raw(
            &blobs,
            account,
            Some(receipts),
            Some(2),
            &fixture("b@x", "a receipt"),
        )
        .unwrap();

    let view = ListView::parse(&format!("folder:{receipts}"));
    let rows = store.list_threads(&view, 0, 50).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].subject, "a receipt");

    // The key is user data on its way in: nonsense falls back to the inbox
    // rather than erroring a whole pane.
    assert_eq!(ListView::parse("folder:nonsense"), ListView::Inbox);
}

#[test]
fn renaming_keeps_the_folder_id_and_its_contents() {
    let (_dir, mut store, blobs, account) = setup();
    let id = store.ensure_named_folder(account, "Reciepts").unwrap();
    store
        .ingest_raw(&blobs, account, Some(id), Some(1), &fixture("r@x", "kept"))
        .unwrap();

    store.rename_folder(id, "Receipts").unwrap();

    let all = store.folders(account).unwrap();
    let row = all.iter().find(|f| f.id == id).expect("still there");
    assert_eq!(row.path, "Receipts");
    // Same id, same contents: the open view survives a rename.
    let rows = store
        .list_threads(&ListView::parse(&format!("folder:{id}")), 0, 50)
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn removing_a_folder_never_removes_the_mail() {
    let (_dir, mut store, blobs, account) = setup();
    let id = store.ensure_named_folder(account, "Doomed").unwrap();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(id),
            Some(1),
            &fixture("d@x", "survivor"),
        )
        .unwrap();

    store.remove_folder(id).unwrap();

    assert!(
        !store.folders(account).unwrap().iter().any(|f| f.id == id),
        "the folder row is gone"
    );
    // The message row and its bytes are not.
    let hash = store
        .blob_hash_for(ingested.message_id)
        .unwrap()
        .expect("message survives");
    assert!(blobs.read(&hash).is_ok(), "bytes survive");
}

#[test]
fn folders_the_server_stopped_listing_are_pruned_without_losing_mail() {
    let (_dir, mut store, blobs, account) = setup();
    // Yesterday's survey: a container, a keeper, and a folder with mail.
    store
        .sync_folders(
            account,
            &[
                ("[Gmail]".into(), None),
                ("Keeper".into(), None),
                ("Doomed".into(), None),
            ],
        )
        .unwrap();
    let all = store.folders(account).unwrap();
    let doomed = all.iter().find(|f| f.path == "Doomed").unwrap().id;
    let keeper = all.iter().find(|f| f.path == "Keeper").unwrap().id;
    let survivor = store
        .ingest_raw(
            &blobs,
            account,
            Some(doomed),
            Some(1),
            &fixture("s@x", "kept"),
        )
        .unwrap();

    // Today's survey: the container is no longer reported (noselect filter)
    // and Doomed was deleted elsewhere.
    store
        .sync_folders(account, &[("Keeper".into(), None)])
        .unwrap();

    let after = store.folders(account).unwrap();
    assert!(after.iter().any(|f| f.id == keeper), "{after:?}");
    assert!(!after.iter().any(|f| f.path == "[Gmail]"), "{after:?}");
    assert!(!after.iter().any(|f| f.id == doomed), "{after:?}");
    // The mail that passed through the pruned folder is still here.
    let hash = store
        .blob_hash_for(survivor.message_id)
        .unwrap()
        .expect("message row survives");
    assert!(blobs.read(&hash).is_ok());
}
