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
    let rows = store
        .list_threads(&view, 0, 50, petrel_engine::store::Sort::default())
        .unwrap();
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
        .list_threads(
            &ListView::parse(&format!("folder:{id}")),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
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

/// A folder whose name contains an underscore is a LIKE pattern, not a string.
///
/// Moving one to the Trash rewrites every descendant path in one statement,
/// matched with `path LIKE old || '/%'`. Underscore matches any single
/// character there, so `a_b` also matched `axb/child` — and the cascade
/// dragged an unrelated folder's subtree into the bin with it. The user's own
/// mailbox has folders named `glassdoor+102025_2`.
#[test]
fn a_name_with_an_underscore_does_not_drag_its_neighbours_along() {
    let (_dir, mut store, _blobs, account) = setup();
    let doomed = store.ensure_named_folder(account, "a_b").unwrap();
    store.ensure_named_folder(account, "axb").unwrap();
    store.ensure_named_folder(account, "axb/child").unwrap();
    store.ensure_named_folder(account, "a_b/child").unwrap();

    store.rename_folder(doomed, "Trash/a_b").unwrap();

    let paths: Vec<String> = store
        .folders(account)
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert!(paths.contains(&"Trash/a_b".to_string()), "the folder moved");
    assert!(
        paths.contains(&"Trash/a_b/child".to_string()),
        "its own child came with it"
    );
    assert!(
        paths.contains(&"axb/child".to_string()),
        "the neighbour stayed put, got: {paths:?}"
    );
}

/// Moving a folder to the Trash must not strand the changes queued for the
/// mail inside it.
///
/// A queued action carries the path its message sat at, so it can still be
/// delivered after a move has deleted the placement holding the UID. The
/// rename made that captured path a name the server no longer had, and the
/// drain then failed with `Mailbox doesn't exist` on every sync cycle — one
/// such action had been retrying in a real mailbox for days.
#[test]
fn binning_a_folder_repoints_the_changes_still_queued_for_its_mail() {
    let (_dir, mut store, blobs, account) = setup();
    let receipts = store.ensure_named_folder(account, "Receipts").unwrap();
    let elsewhere = store.ensure_named_folder(account, "Elsewhere").unwrap();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(receipts),
            Some(41),
            &fixture("q@x", "queued"),
        )
        .unwrap();
    let thread = store
        .thread_of(ingested.message_id)
        .unwrap()
        .unwrap_or(-ingested.message_id);

    // The move deletes the placement, so delivery has only the captured path.
    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::Move,
            Some(elsewhere),
            petrel_engine::actions::PlacementPolicy::Exclusive,
        )
        .unwrap();
    let queued = store.pending_actions(account).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].folder_path, "Receipts");
    assert_eq!(queued[0].uid, Some(41));

    store.rename_folder(receipts, "Trash/Receipts").unwrap();

    let after = store.pending_actions(account).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].folder_path, "Trash/Receipts",
        "the queue follows the folder, or the change is never delivered"
    );
    assert_eq!(after[0].uid, Some(41), "and still knows where to aim");
}

/// Archiving on an account whose server has no Archive folder.
///
/// `ensure_folder(account, "archive", "archive")` invents a local folder when
/// nothing wears the role — and the next survey prunes every folder the server
/// did not list, which that one never will. With an exclusive placement policy
/// the archived message lives *only* there, so pruning tombstones it: gone
/// from every view and from search, by pressing Archive and waiting.
///
/// Namecheap marks no \Archive. The account this was written against has
/// 10,479 messages filed under a plain `Archive` folder and no archive role.
#[test]
fn archiving_without_a_server_archive_folder_does_not_lose_the_mail() {
    let (_dir, mut store, blobs, account) = setup();
    // The mailbox as the server reports it: an inbox, and a plain folder the
    // person files into. No folder wears the archive role.
    store
        .sync_folders(account, &[("INBOX".into(), None), ("Archive".into(), None)])
        .unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(7),
            &fixture("a@x", "keep me"),
        )
        .unwrap();
    let thread = store
        .thread_of(ingested.message_id)
        .unwrap()
        .unwrap_or(-ingested.message_id);

    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::Archive,
            None,
            petrel_engine::actions::PlacementPolicy::Exclusive,
        )
        .unwrap();

    // The next survey. The server still reports what it always did.
    store
        .sync_folders(account, &[("INBOX".into(), None), ("Archive".into(), None)])
        .unwrap();

    // Tombstoning drops a message out of search, which is the observable a
    // person would actually meet: the mail is not anywhere they can look.
    let hits = store.search("keep", 10).unwrap();
    assert_eq!(
        hits.len(),
        1,
        "archiving then syncing must not make the message unfindable"
    );
}

/// The Archive mailbox on an account whose server marks no \Archive.
///
/// Namecheap flags \Sent, \Trash, \Drafts and \Junk and nothing else, so the
/// archive view's predicate — which asks for `role = 'archive'` — matched
/// nothing and the mailbox listed zero while the plain `Archive` folder below
/// it held ten thousand messages.
#[test]
fn a_plain_archive_folder_is_the_archive() {
    let (_dir, mut store, blobs, account) = setup();
    store
        .sync_folders(
            account,
            &[
                ("INBOX".into(), Some("inbox".into())),
                ("Sent".into(), Some("sent".into())),
                // No role. This is the whole point.
                ("Archive".into(), None),
                ("Archive/2026".into(), None),
            ],
        )
        .unwrap();

    let folders = store.folders(account).unwrap();
    let archive = folders.iter().find(|f| f.path == "Archive").unwrap();
    assert_eq!(archive.role, "archive", "the plain folder wears the role");

    // And the view built on that role finds what is filed under it.
    let nested = folders
        .iter()
        .find(|f| f.path == "Archive/2026")
        .unwrap()
        .id;
    store
        .ingest_raw(
            &blobs,
            account,
            Some(nested),
            Some(3),
            &fixture("f@x", "filed"),
        )
        .unwrap();
    assert_eq!(
        store
            .conversations_in(&ListView::Folder("archive".into()))
            .unwrap(),
        1,
        "the archive mailbox lists what is filed beneath it"
    );

    // A later survey still reporting no flag must not take the role away.
    store
        .sync_folders(
            account,
            &[
                ("INBOX".into(), Some("inbox".into())),
                ("Sent".into(), Some("sent".into())),
                ("Archive".into(), None),
                ("Archive/2026".into(), None),
            ],
        )
        .unwrap();
    let after = store.folders(account).unwrap();
    let archive = after.iter().find(|f| f.path == "Archive").unwrap();
    assert_eq!(archive.role, "archive", "and keeps it across surveys");
}

/// Marking a whole folder read, and back again.
///
/// One statement rather than a loop, because a real folder holds ten thousand
/// messages and this runs while somebody watches the sidebar. What the test
/// pins is the arithmetic: only the rows that actually changed are counted, so
/// the app can say "nothing to do" rather than claiming it marked a folder
/// that was already read.
#[test]
fn marking_a_folder_read_touches_only_what_was_unread() {
    let (_dir, mut store, blobs, account) = setup();
    let folder = store.ensure_named_folder(account, "Newsletters").unwrap();
    let other = store.ensure_named_folder(account, "Elsewhere").unwrap();
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(
            store
                .ingest_raw(
                    &blobs,
                    account,
                    Some(folder),
                    Some(i + 1),
                    &fixture(&format!("n{i}@x"), "note"),
                )
                .unwrap()
                .message_id,
        );
    }
    // One elsewhere, which must not be touched.
    let outsider = store
        .ingest_raw(
            &blobs,
            account,
            Some(other),
            Some(9),
            &fixture("out@x", "outside"),
        )
        .unwrap()
        .message_id;

    // Ingested mail is unread, so all three change.
    assert_eq!(store.mark_folder_seen(folder, true).unwrap(), 3);
    // And a second pass changes nothing, rather than reporting three again.
    assert_eq!(store.mark_folder_seen(folder, true).unwrap(), 0);

    let seen =
        |id: i64| -> bool { store.flags_of(id).unwrap() & petrel_engine::store::flags::SEEN != 0 };
    assert!(
        ids.iter().all(|id| seen(*id)),
        "every message in the folder"
    );
    assert!(!seen(outsider), "and nothing outside it");

    // Back again.
    assert_eq!(store.mark_folder_seen(folder, false).unwrap(), 3);
    assert!(ids.iter().all(|id| !seen(*id)));
}

/// Emptying a folder into the bin.
///
/// Exclusive, like every other binning: the mail is in the Trash and nowhere
/// else. Mail that reads as deleted and is still filed in two places is the
/// state this avoids.
#[test]
fn moving_a_folders_contents_leaves_them_only_in_the_bin() {
    let (_dir, mut store, blobs, account) = setup();
    let folder = store.ensure_named_folder(account, "Doomed").unwrap();
    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let m = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(1),
            &fixture("d@x", "doomed"),
        )
        .unwrap()
        .message_id;
    // Also filed in the inbox, the way a Gmail message carries two labels.
    store.place_message(m, inbox).unwrap();

    assert_eq!(store.folder_message_count(folder).unwrap(), 1);
    assert_eq!(store.move_folder_contents(folder, trash).unwrap(), 1);

    assert_eq!(
        store.folders_of(m).unwrap(),
        vec![trash],
        "in the bin, and not still in the folder or the inbox"
    );
    assert_eq!(store.folder_message_count(folder).unwrap(), 0);
    // The message itself survives; only its filing changed.
    assert!(store.blob_hash_for(m).unwrap().is_some());
}

/// "All the mail in here" means the subtree, not the one mailbox.
///
/// Caught by measuring rather than by reading: on a real account `Archive`
/// itself holds a single message while `Archive/...` holds ten thousand, so a
/// Mark all as read that stopped at the named folder reported marking one and
/// looked broken. Empty Trash already read the subtree; these had to follow.
#[test]
fn marking_and_binning_a_folder_reach_everything_filed_under_it() {
    let (_dir, mut store, blobs, account) = setup();
    let parent = store.ensure_named_folder(account, "Archive").unwrap();
    let child = store.ensure_named_folder(account, "Archive/2026").unwrap();
    let deep = store
        .ensure_named_folder(account, "Archive/2026/Q1")
        .unwrap();
    // A folder that merely starts with the same letters must not be swept in.
    let bystander = store
        .ensure_named_folder(account, "Archived stuff")
        .unwrap();

    let mut n = 0;
    for folder in [parent, child, deep, bystander] {
        n += 1;
        store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(n),
                &fixture(&format!("m{n}@x"), "note"),
            )
            .unwrap();
    }

    assert_eq!(
        store.folder_message_count(parent).unwrap(),
        3,
        "the folder and its two descendants, not the lookalike beside them"
    );
    assert_eq!(store.mark_folder_seen(parent, true).unwrap(), 3);

    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    assert_eq!(store.move_folder_contents(parent, trash).unwrap(), 3);
    assert_eq!(store.folder_message_count(parent).unwrap(), 0);
    assert_eq!(
        store.folder_message_count(bystander).unwrap(),
        1,
        "`Archived stuff` is not under `Archive`"
    );
}
