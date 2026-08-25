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

#[test]
fn the_backfill_cursor_survives_and_finishes() {
    let (_dir, mut store, blobs, account) = setup();
    let folder = store.ensure_named_folder(account, "History").unwrap();
    // Nothing held, nothing walked: no cursor at all.
    assert_eq!(store.min_uid(folder).unwrap(), None);
    assert_eq!(store.backfill_floor(folder).unwrap(), None);

    // The seed took uids 90 and 100; the walk starts below 90.
    for uid in [90, 100] {
        store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(uid),
                &fixture(&format!("u{uid}@x"), "old"),
            )
            .unwrap();
    }
    assert_eq!(store.min_uid(folder).unwrap(), Some(90));

    // Strides record how deep they asked, not how much they got — a stretch
    // emptied by expunges must not be asked about twice.
    store.set_backfill_floor(folder, 50).unwrap();
    assert_eq!(store.backfill_floor(folder).unwrap(), Some(50));
    store.set_backfill_floor(folder, 1).unwrap();
    assert_eq!(
        store.backfill_floor(folder).unwrap(),
        Some(1),
        "floor 1 is done"
    );

    // It shares sync_state_json with the other cursors without clobbering them.
    store.set_folder_modseq(folder, 77).unwrap();
    assert_eq!(store.backfill_floor(folder).unwrap(), Some(1));
    assert_eq!(store.folder_modseq(folder).unwrap(), Some(77));
}

#[test]
fn renaming_a_parent_carries_its_subtree() {
    let (_dir, mut store, blobs, account) = setup();
    let parent = store.ensure_named_folder(account, "Projects").unwrap();
    let child = store
        .ensure_named_folder(account, "Projects/Petrel")
        .unwrap();
    let grand = store
        .ensure_named_folder(account, "Projects/Petrel/Specs")
        .unwrap();
    let stranger = store.ensure_named_folder(account, "Projectsong").unwrap();
    store
        .ingest_raw(
            &blobs,
            account,
            Some(grand),
            Some(1),
            &fixture("g@x", "kept"),
        )
        .unwrap();

    // Nesting-by-rename: the whole point of rename being IMAP's move.
    store.rename_folder(parent, "Archive/Projects").unwrap();

    let all = store.folders(account).unwrap();
    let path = |id| all.iter().find(|f| f.id == id).unwrap().path.clone();
    assert_eq!(path(parent), "Archive/Projects");
    assert_eq!(path(child), "Archive/Projects/Petrel");
    assert_eq!(path(grand), "Archive/Projects/Petrel/Specs");
    // A name that merely starts the same is not a descendant.
    assert_eq!(path(stranger), "Projectsong");
    // Ids never changed, so the grandchild's mail is untouched.
    assert_eq!(store.max_uid(grand).unwrap(), Some(1));
}
