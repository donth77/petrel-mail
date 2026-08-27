//! Retention policy in practice (Q24).
//!
//! Three promises are made to the user, and each is a test here:
//! * delete elsewhere and it disappears here — including from search;
//! * a deletion stays recoverable for the grace period;
//! * a local archive is not touched by the server at all.
//!
//! The fourth test is the one that matters most: **after the grace period the
//! bytes are actually gone from disk.** "Deleted" that leaves a readable copy
//! in the user's home directory is not deletion.

use petrel_engine::blob::BlobStore;
use petrel_engine::retention::{DEFAULT_GRACE_DAYS, MS_PER_DAY, RetentionMode};
use petrel_engine::store::Store;

const NOW: i64 = 1_800_000_000_000;

fn msg(id: &str, body: &str) -> Vec<u8> {
    format!(
        "From: someone@example.com\r\n\
         To: me@example.com\r\n\
         Subject: message {id}\r\n\
         Message-ID: <{id}@example.com>\r\n\r\n\
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
fn mirror_mode_removes_mail_deleted_on_the_server() {
    let (_d, mut store, blobs, account) = setup();
    for id in ["keep-1", "delete-me"] {
        let raw = msg(id, "shared body text kingfisher");
        store
            .ingest_raw(&blobs, account, None, None, &raw)
            .expect("ingest");
    }
    assert_eq!(store.search("kingfisher", 10).expect("search").len(), 2);

    // The server now lists only one of them.
    let removed = store
        .reconcile_server_absences(account, &["keep-1@example.com".to_string()], NOW)
        .expect("reconcile");
    assert_eq!(removed, 1);

    // Gone from every view the user has — search included. A "deleted" message
    // that still turns up in results is the bug this guards.
    let hits = store.search("kingfisher", 10).expect("search");
    assert_eq!(hits.len(), 1, "deleted mail must not remain searchable");
    assert_eq!(store.list_recent(0, 50).expect("list").len(), 1);
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn a_deletion_is_recoverable_during_the_grace_period() {
    let (_d, mut store, blobs, account) = setup();
    let out = store
        .ingest_raw(
            &blobs,
            account,
            None,
            None,
            &msg("oops", "important wolverine notes"),
        )
        .expect("ingest");

    store
        .reconcile_server_absences(account, &[], NOW)
        .expect("reconcile");
    assert!(store.search("wolverine", 10).expect("search").is_empty());

    // Still on disk, still restorable — this is what the grace period buys.
    assert!(
        store
            .restore_message(&blobs, out.message_id)
            .expect("restore"),
        "a deletion inside the window must be undoable"
    );
    let hits = store.search("wolverine", 10).expect("search");
    assert_eq!(hits.len(), 1, "restored mail returns to the index");
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn after_the_grace_period_the_bytes_are_really_gone() {
    let (_d, mut store, blobs, account) = setup();
    let out = store
        .ingest_raw(
            &blobs,
            account,
            None,
            None,
            &msg("goodbye", "sensitive badger content"),
        )
        .expect("ingest");
    let hash = out.blob_hash.clone();
    assert!(blobs.is_intact(&hash));

    store
        .reconcile_server_absences(account, &[], NOW)
        .expect("reconcile");

    // Inside the window: nothing is destroyed yet.
    let early = store
        .gc(&blobs, NOW + 29 * MS_PER_DAY, DEFAULT_GRACE_DAYS)
        .expect("gc");
    assert_eq!(early.messages_purged, 0);
    assert!(
        blobs.is_intact(&hash),
        "still recoverable inside the window"
    );

    // Past it: the row and the file both go. Anything less would mean the user
    // deleted mail that quietly persists in their home directory.
    let late = store
        .gc(&blobs, NOW + 31 * MS_PER_DAY, DEFAULT_GRACE_DAYS)
        .expect("gc");
    assert_eq!(late.messages_purged, 1);
    assert_eq!(late.blobs_removed, 1);
    assert!(
        !blobs.is_intact(&hash),
        "deleted must mean the bytes are gone"
    );
    assert_eq!(store.message_count().expect("count"), 0);
    assert!(
        !store
            .restore_message(&blobs, out.message_id)
            .expect("restore"),
        "nothing to restore once purged"
    );
}

#[test]
fn local_archive_survives_the_server_deleting_everything() {
    let (_d, mut store, blobs, account) = setup();
    store
        .set_local_archive(account, true)
        .expect("enable archive");
    assert_eq!(
        store.retention_mode(account).expect("mode"),
        RetentionMode::LocalArchive
    );

    for id in ["a", "b", "c"] {
        store
            .ingest_raw(
                &blobs,
                account,
                None,
                None,
                &msg(id, "archived pelican records"),
            )
            .expect("ingest");
    }

    // The account is wiped upstream — suspension, closure, or a bad actor.
    let removed = store
        .reconcile_server_absences(account, &[], NOW)
        .expect("reconcile");
    assert_eq!(
        removed, 0,
        "archive mode must not follow the server's deletions"
    );

    assert_eq!(store.search("pelican", 10).expect("search").len(), 3);

    // And GC must not quietly undo the archive later.
    let report = store
        .gc(&blobs, NOW + 365 * MS_PER_DAY, DEFAULT_GRACE_DAYS)
        .expect("gc");
    assert_eq!(report.messages_purged, 0);
    assert_eq!(store.message_count().expect("count"), 3);
}

#[test]
fn gc_never_reclaims_a_blob_another_message_still_uses() {
    let (_d, mut store, blobs, account) = setup();
    // Identical bytes ingested under two accounts share one blob by content
    // hash — deleting one must not blank the other.
    let second = store.ensure_test_account().expect("second account");
    let raw = msg("shared", "identical body osprey");
    let a = store
        .ingest_raw(&blobs, account, None, None, &raw)
        .expect("a");
    let b = store
        .ingest_raw(&blobs, second, None, None, &raw)
        .expect("b");
    assert_eq!(a.blob_hash, b.blob_hash, "same bytes dedupe to one blob");

    store
        .reconcile_server_absences(account, &[], NOW)
        .expect("reconcile");
    let report = store
        .gc(&blobs, NOW + 31 * MS_PER_DAY, DEFAULT_GRACE_DAYS)
        .expect("gc");

    assert_eq!(report.messages_purged, 1);
    assert_eq!(report.blobs_removed, 0, "blob is still referenced");
    assert!(
        blobs.is_intact(&a.blob_hash),
        "the surviving account's message must still be readable"
    );
    assert_eq!(store.search("osprey", 10).expect("search").len(), 1);
}

#[test]
fn gc_is_idempotent() {
    let (_d, mut store, blobs, account) = setup();
    store
        .ingest_raw(&blobs, account, None, None, &msg("x", "transient content"))
        .expect("ingest");
    store
        .reconcile_server_absences(account, &[], NOW)
        .expect("reconcile");

    let first = store
        .gc(&blobs, NOW + 31 * MS_PER_DAY, DEFAULT_GRACE_DAYS)
        .expect("gc");
    let second = store
        .gc(&blobs, NOW + 31 * MS_PER_DAY, DEFAULT_GRACE_DAYS)
        .expect("gc");
    assert_eq!(first.messages_purged, 1);
    assert_eq!(second.messages_purged, 0, "a second pass must find nothing");
    assert_eq!(second.blobs_removed, 0);
}

#[test]
fn reconciling_an_unchanged_mailbox_deletes_nothing() {
    // The dangerous failure mode: a sync bug that reports an empty server and
    // wipes the user's mail. Present-set reconciliation must be conservative.
    let (_d, mut store, blobs, account) = setup();
    let mut ids = Vec::new();
    for id in ["m1", "m2", "m3"] {
        store
            .ingest_raw(&blobs, account, None, None, &msg(id, "steady state heron"))
            .expect("ingest");
        ids.push(format!("{id}@example.com"));
    }

    let removed = store
        .reconcile_server_absences(account, &ids, NOW)
        .expect("reconcile");
    assert_eq!(removed, 0);
    assert_eq!(store.search("heron", 10).expect("search").len(), 3);
}

#[test]
fn gc_retires_queued_actions_that_can_never_deliver() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    // An action row with no action_messages rows — the shape rows took
    // before that table existed. pending_actions can never list it.
    store
        .plant_orphan_action_for_tests(account)
        .expect("plant orphan");
    let report = store.gc(&blobs, 1, 30).expect("gc");
    assert_eq!(report.actions_orphaned, 1);
    // Run twice: the rename sticks and is not recounted.
    let again = store.gc(&blobs, 1, 30).expect("gc again");
    assert_eq!(again.actions_orphaned, 0);
}

#[test]
fn deleting_a_folder_takes_the_mail_that_lived_only_there() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let doomed = store
        .ensure_folder(account, "", "Trash/old")
        .expect("folder");
    let keep = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    let raw = |mid: &str| {
        format!(
            "From: a@example.com\r\nTo: me@example.com\r\nSubject: {mid}\r\n\
             Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <{mid}>\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n"
        )
        .into_bytes()
    };
    // One message only in the doomed folder; one also filed in the inbox.
    let only = store
        .ingest_raw(&blobs, account, Some(doomed), Some(1), &raw("only@x"))
        .expect("ingest");
    let both = store
        .ingest_raw(&blobs, account, Some(doomed), Some(2), &raw("both@x"))
        .expect("ingest");
    store
        .place_message_at(both.message_id, keep, 9)
        .expect("place");

    let took = store.remove_folder(doomed).expect("remove");
    assert_eq!(took, 1, "only the message with nowhere else to be");

    // The one that lived only there is gone from search and marked for the
    // grace-period sweep — not left haunting the store with no folder.
    assert!(
        store.search("only@x", 10).expect("search").is_empty(),
        "a deleted message must stop answering searches"
    );
    // The one that also lives in the inbox is untouched.
    assert_eq!(
        store.folders_of(both.message_id).expect("folders"),
        vec![keep]
    );
    assert_eq!(
        store.search("both@x", 10).expect("search").len(),
        1,
        "mail that lives somewhere else is not deleted by a folder going away"
    );
    let _ = only;
}

#[test]
fn the_bin_measures_time_in_the_bin_not_the_age_of_the_mail() {
    const DAY: i64 = 24 * 60 * 60 * 1000;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let trash = store
        .ensure_folder(account, "trash", "Trash")
        .expect("trash");
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("inbox");
    // A two-year-old receipt: old mail, but only just binned.
    let raw = b"From: shop@example.com\r\nTo: me@example.com\r\nSubject: receipt\r\n\
                Date: Tue, 18 Aug 2024 14:02:00 +0000\r\nMessage-ID: <old@x>\r\n\
                MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n";
    let old = store
        .ingest_raw(&blobs, account, Some(trash), Some(1), raw)
        .expect("ingest");
    let now = 1_800_000_000_000i64;
    store.refresh_trash_clock(account, now).expect("clock");

    // Just arrived in the bin: a thirty-day rule must not touch it, however
    // old the message itself is.
    assert!(
        store
            .trash_expired(account, 30, now)
            .expect("expired")
            .is_empty(),
        "the clock starts at the bin, not at the postmark"
    );
    // Thirty-one days later it goes.
    let later = now + 31 * DAY;
    let due = store.trash_expired(account, 30, later).expect("expired");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].2, old.message_id);

    // Taken back out of the bin, the clock stops and resets.
    store
        .place_message_at(old.message_id, inbox, 5)
        .expect("restore");
    store
        .remove_placement(old.message_id, account, "Trash")
        .expect("unbin");
    store.refresh_trash_clock(account, later).expect("clock");
    assert!(
        store
            .trash_expired(account, 30, later + 99 * DAY)
            .expect("expired")
            .is_empty(),
        "mail rescued from the bin is not still on its clock"
    );
}
