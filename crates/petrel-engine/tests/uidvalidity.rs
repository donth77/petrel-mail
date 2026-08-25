//! UIDVALIDITY reset: the server renumbered a folder, every stored UID is a
//! lie, and the mend must cost at worst a re-download — never data.
//!
//! The properties pinned here: quarantine holds queued actions off the wire
//! (a NULL UID is already "not addressable"), the re-map learns new UIDs by
//! the same key ingest dedupes on, an incomplete listing never evicts
//! history, and no message row or blob is ever deleted.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn fixture(message_id: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{message_id}>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         body of {subject}\r\n"
    )
    .into_bytes()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let folder = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("folder");
    (dir, store, blobs, account, folder)
}

#[test]
fn validity_is_recorded_and_read_back() {
    let (_dir, mut store, _blobs, _account, folder) = setup();
    assert_eq!(store.folder_validity(folder).unwrap(), None);
    store.set_folder_validity(folder, Some(1111)).unwrap();
    assert_eq!(store.folder_validity(folder).unwrap(), Some(1111));
}

#[test]
fn a_reset_remaps_by_message_id_and_refetches_only_strangers() {
    let (_dir, mut store, blobs, account, folder) = setup();
    // Three messages synced under the old numbering: UIDs 101, 102, 103.
    for (uid, mid, subj) in [
        (101, "alpha@x", "alpha"),
        (102, "beta@x", "beta"),
        (103, "gamma@x", "gamma"),
    ] {
        store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(uid),
                &fixture(mid, subj),
            )
            .expect("ingest");
    }
    store.set_folder_validity(folder, Some(1)).unwrap();

    // The server renumbered: alpha is now 1, gamma is 3, beta is gone from
    // the folder, and a message we never saw (delta) is 4.
    let server = [
        (1u32, Some("alpha@x".to_string())),
        (3, Some("gamma@x".to_string())),
        (4, Some("delta@x".to_string())),
    ];
    let out = store
        .remap_folder_after_reset(folder, &server, true)
        .expect("remap");

    assert_eq!(out.rematched, 2);
    assert_eq!(out.to_fetch, vec![4], "only the stranger is re-downloaded");
    assert_eq!(out.dropped, 1, "beta left the folder");

    // The watermark now speaks the new numbering.
    assert_eq!(store.max_uid(folder).unwrap(), Some(3));
}

#[test]
fn an_incomplete_listing_never_evicts_history() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let mut ids = Vec::new();
    for (uid, mid, subj) in [(500, "old@x", "old"), (900, "new@x", "new")] {
        let out = store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(uid),
                &fixture(mid, subj),
            )
            .expect("ingest");
        ids.push(out.message_id);
    }
    // A depth-limited listing that only reached the newest message.
    let server = [(2u32, Some("new@x".to_string()))];
    let out = store
        .remap_folder_after_reset(folder, &server, false)
        .expect("remap");
    assert_eq!(out.rematched, 1);
    assert_eq!(out.dropped, 0, "the window ending is not the mail ending");

    // The old message is still placed in the folder — just not addressable
    // until a deeper pass learns its new number.
    // Both messages still live in the folder: max_uid sees the remapped one,
    // and the held one keeps its placement (folders_of still names the folder).
    assert_eq!(store.max_uid(folder).unwrap(), Some(2));
    for id in ids {
        assert_eq!(store.folders_of(id).unwrap(), vec![folder]);
    }
}

#[test]
fn quarantine_holds_queued_actions_rather_than_misfiring_them() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(77),
            &fixture("held@x", "held"),
        )
        .expect("ingest");
    let thread = store
        .thread_of(ingested.message_id)
        .expect("thread lookup")
        .expect("threaded");
    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::MarkRead,
            None,
            store.placement_policy(account).expect("policy"),
        )
        .expect("queue");

    // Before the reset the action is deliverable — it knows its UID.
    let before = store.pending_actions(account).expect("pending");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].uid, Some(77));

    // Renumbered, and the listing did not include this message.
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    // The action still exists but no longer names a UID: the drain loop's
    // existing rule for a NULL UID is to hold, so nothing fires at whatever
    // message inherited number 77 on the server.
    let after = store.pending_actions(account).expect("pending");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].uid, None);
}

#[test]
fn recovery_never_deletes_a_blob_or_a_message() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(11),
            &fixture("kept@x", "kept"),
        )
        .expect("ingest");
    // Complete listing, and the server no longer has the message at all.
    let out = store
        .remap_folder_after_reset(folder, &[], true)
        .expect("remap");
    assert_eq!(out.dropped, 1);

    // The placement is gone; the message and its bytes are not. Worst case
    // costs a re-download, never data.
    let hash = store
        .blob_hash_for(a.message_id)
        .expect("query")
        .expect("message row survives");
    assert!(
        blobs.read(&hash).is_ok(),
        "the raw bytes must still be readable"
    );
}

#[test]
fn a_quarantined_action_still_names_its_message_and_where_to_ask() {
    // Quarantine nulls the UID, and that must hold — but the action is not
    // thereby lost. The pending row carries the Message-ID and the folder a
    // drain can search, so delivery can ask the server for the new number
    // instead of holding the change forever.
    let (_dir, mut store, blobs, account, folder) = setup();
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(77),
            &fixture("held@x", "held"),
        )
        .expect("ingest");
    let thread = store
        .thread_of(ingested.message_id)
        .expect("thread lookup")
        .expect("threaded");
    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::MarkRead,
            None,
            store.placement_policy(account).expect("policy"),
        )
        .expect("queue");
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    let after = store.pending_actions(account).expect("pending");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].uid, None, "quarantine still holds the number");
    assert_eq!(after[0].msgid.as_deref(), Some("held@x"));
    assert_eq!(after[0].candidate_paths, vec!["INBOX".to_string()]);

    // A deliverable action asks nothing: known UID, no candidates.
    let fresh = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(90),
            &fixture("fresh@x", "fresh"),
        )
        .expect("ingest");
    let t2 = store.thread_of(fresh.message_id).unwrap().unwrap();
    store
        .apply_thread_action(
            account,
            t2,
            petrel_engine::actions::ActionKind::Star,
            None,
            store.placement_policy(account).expect("policy"),
        )
        .expect("queue");
    let again = store.pending_actions(account).expect("pending");
    let starred = again.iter().find(|a| a.uid == Some(90)).expect("fresh row");
    assert!(starred.candidate_paths.is_empty());
}

#[test]
fn healing_fills_only_the_number_that_was_lost() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(41),
            &fixture("a@x", "a"),
        )
        .expect("ingest");
    store
        .remap_folder_after_reset(folder, &[], false)
        .expect("remap");

    // The server, asked by Message-ID, says the message is now UID 900.
    assert!(
        store
            .heal_placement_uid(a.message_id, account, "INBOX", 900)
            .expect("heal")
    );
    assert_eq!(store.max_uid(folder).unwrap(), Some(900));

    // A placement already holding a UID is the sync's business: healing it
    // is refused rather than applied.
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(50),
            &fixture("b@x", "b"),
        )
        .expect("ingest");
    assert!(
        !store
            .heal_placement_uid(b.message_id, account, "INBOX", 999)
            .expect("no-op heal")
    );
}

#[test]
fn the_sweep_removes_only_what_the_server_no_longer_holds() {
    let (_dir, mut store, blobs, account, folder) = setup();
    for (uid, mid) in [(11, "a@x"), (12, "b@x"), (13, "c@x")] {
        store
            .ingest_raw(&blobs, account, Some(folder), Some(uid), &fixture(mid, mid))
            .expect("ingest");
    }
    // One placement quarantined: its number is NULL, and no server answer
    // about live UIDs says anything about it.
    let held = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(14),
            &fixture("d@x", "d"),
        )
        .expect("ingest");
    store
        .remap_folder_after_reset(
            folder,
            &[
                (11, Some("a@x".into())),
                (12, Some("b@x".into())),
                (13, Some("c@x".into())),
            ],
            // Incomplete, so the unlisted fourth message is held, not evicted.
            false,
        )
        .expect("remap");
    assert_eq!(store.uid_placement_count(folder).unwrap(), 3);

    // The server now holds only 11 and 13: 12 was moved elsewhere.
    let present: std::collections::HashSet<u32> = [11, 13].into_iter().collect();
    let removed = store
        .remove_placements_absent(folder, &present)
        .expect("sweep");
    assert_eq!(removed, 1);
    assert_eq!(store.uid_placement_count(folder).unwrap(), 2);
    // The quarantined message keeps its placement.
    assert_eq!(store.folders_of(held.message_id).unwrap(), vec![folder]);
}

#[test]
fn a_delivered_move_takes_the_source_placement_with_it() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(21),
            &fixture("m@x", "m"),
        )
        .expect("ingest");
    assert!(
        store
            .remove_placement(a.message_id, account, "INBOX")
            .expect("remove")
    );
    assert!(store.folders_of(a.message_id).unwrap().is_empty());
    // Absent placement: nothing to remove, and saying so is not an error.
    assert!(
        !store
            .remove_placement(a.message_id, account, "INBOX")
            .expect("no-op")
    );
}

#[test]
fn a_second_live_copy_is_its_own_message() {
    let (_dir, mut store, blobs, account, folder) = setup();
    let first = store
        .ingest_raw(
            &blobs,
            account,
            Some(folder),
            Some(70),
            &fixture("twice@x", "draft one"),
        )
        .expect("ingest");
    // Same Message-ID, different content, both live on the server: the
    // second copy keeps its own row and placement rather than overwriting
    // the first.
    let raw2 = fixture("twice@x", "draft two");
    let second = store
        .ingest_raw_second_copy(&blobs, account, Some(folder), 71, &raw2)
        .expect("second copy");
    assert_ne!(first.message_id, second.message_id);
    assert_eq!(store.uid_placement_count(folder).unwrap(), 2);
    // A refetch of the same copy lands on the same row, not a third.
    let again = store
        .ingest_raw_second_copy(&blobs, account, Some(folder), 71, &raw2)
        .expect("refetch");
    assert_eq!(again.message_id, second.message_id);
    assert_eq!(store.uid_placement_count(folder).unwrap(), 2);
}
