//! Mail that leaves the server leaves here too — when the account mirrors
//! the server, and only when leaving the folder means leaving the server.
//!
//! The ghost this pins: a message whose last placement the sweep dropped
//! used to stay live with no folder at all — out of every view, still
//! answering searches, still in its conversation, counted in the account
//! total, and never collected.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Sort, Store};
use std::collections::HashSet;

const DAY: i64 = 24 * 60 * 60 * 1000;
const T0: i64 = 1_800_000_000_000;

fn mail(msgid: &str, subject: &str, refs: &[&str], body: &str) -> Vec<u8> {
    let mut headers = format!(
        "From: Someone <someone@example.com>\r\nTo: me@example.com\r\n\
         Subject: {subject}\r\nMessage-ID: <{msgid}>\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n"
    );
    if !refs.is_empty() {
        let list: Vec<String> = refs.iter().map(|r| format!("<{r}>")).collect();
        headers.push_str(&format!("References: {}\r\n", list.join(" ")));
    }
    format!("{headers}\r\n{body}\r\n").into_bytes()
}

fn setup() -> (Store, BlobStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let store = Store::open_in_memory().unwrap();
    (store, blobs, dir)
}

fn present(uids: &[u32]) -> HashSet<u32> {
    uids.iter().copied().collect()
}

#[test]
fn the_sweep_tombstones_a_message_it_leaves_with_no_folder() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let root = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("r@x", "Capybara plans", &[], "capybara root"),
        )
        .unwrap();
    let reply = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(2),
            &mail("p@x", "Re: Capybara plans", &["r@x"], "capybara reply"),
        )
        .unwrap();
    let thread = store.thread_of(reply.message_id).unwrap().unwrap();

    // The phone deleted the reply: the server's INBOX holds UID 1 only.
    assert_eq!(
        store
            .remove_placements_absent(inbox, &present(&[1]))
            .unwrap(),
        1
    );
    assert!(store.folders_of(reply.message_id).unwrap().is_empty());

    // Gone from search, from the conversation, and from the account total.
    assert!(
        store
            .search_threads("capybara reply", 50)
            .unwrap()
            .is_empty(),
        "a deleted message must not answer searches"
    );
    let index = store.thread_index(thread).unwrap();
    assert!(index.iter().all(|r| r.id != reply.message_id));
    assert_eq!(store.accounts().unwrap()[0].message_count, 1);
    assert!(
        store.search_threads("capybara root", 50).unwrap().len() == 1,
        "the message the server still holds is untouched"
    );
    assert_eq!(store.folders_of(root.message_id).unwrap(), vec![inbox]);

    // And collected once the grace period is over.
    let report = store.gc(&blobs, T0 + 400 * DAY, 30).unwrap();
    assert_eq!(report.messages_purged, 1);
}

#[test]
fn a_message_still_held_elsewhere_is_not_tombstoned() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let work = store.ensure_named_folder(a, "Work").unwrap();
    let m = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("w@x", "Wombat", &[], "wombat text"),
        )
        .unwrap();
    store.place_message_at(m.message_id, work, 7).unwrap();

    assert_eq!(
        store
            .remove_placements_absent(inbox, &present(&[]))
            .unwrap(),
        1
    );
    assert_eq!(store.folders_of(m.message_id).unwrap(), vec![work]);
    assert_eq!(store.search_threads("wombat", 50).unwrap().len(), 1);
}

#[test]
fn a_local_archive_keeps_what_the_server_dropped() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    store.set_local_archive(a, true).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let m = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("k@x", "Kept", &[], "kept forever"),
        )
        .unwrap();
    assert_eq!(
        store
            .remove_placements_absent(inbox, &present(&[]))
            .unwrap(),
        1
    );
    assert!(store.folders_of(m.message_id).unwrap().is_empty());
    assert_eq!(
        store.search_threads("kept forever", 50).unwrap().len(),
        1,
        "keeping it is the point of the mode"
    );
    assert_eq!(
        store
            .gc(&blobs, T0 + 400 * DAY, 30)
            .unwrap()
            .messages_purged,
        0
    );
}

/// Where folders are labels, leaving INBOX is archiving, not deleting: the
/// message is still in All Mail whether or not the walk has claimed it yet.
#[test]
fn leaving_a_label_is_not_leaving_the_server() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    store.set_account_kind(a, "gmail").unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let all = store
        .ensure_folder(a, "archive", "[Gmail]/All Mail")
        .unwrap();
    let m = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("g@x", "Gmail", &[], "archived on the phone"),
        )
        .unwrap();
    assert_eq!(
        store
            .remove_placements_absent(inbox, &present(&[]))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .search_threads("archived on the phone", 50)
            .unwrap()
            .len(),
        1,
        "still on the server, so still here"
    );

    // The bin is where Gmail mail actually leaves from.
    let trash = store.ensure_folder(a, "trash", "[Gmail]/Trash").unwrap();
    store.place_message_at(m.message_id, trash, 4).unwrap();
    assert_eq!(
        store
            .remove_placements_absent(trash, &present(&[]))
            .unwrap(),
        1
    );
    assert!(
        store
            .search_threads("archived on the phone", 50)
            .unwrap()
            .is_empty(),
        "expunged from the bin is gone"
    );
    let _ = all;
}

#[test]
fn a_resync_brings_the_message_back() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let receipts = store.ensure_named_folder(a, "Receipts").unwrap();
    let raw = mail("mv@x", "Moved on the phone", &[], "moved elsewhere");
    let m = store
        .ingest_raw(&blobs, a, Some(inbox), Some(1), &raw)
        .unwrap();
    store
        .remove_placements_absent(inbox, &present(&[]))
        .unwrap();
    assert!(
        store
            .search_threads("moved elsewhere", 50)
            .unwrap()
            .is_empty()
    );

    // The folder it went to syncs, and the same message comes back under a
    // new number: the tombstone clears and the row is the same row.
    let again = store
        .ingest_raw(&blobs, a, Some(receipts), Some(9), &raw)
        .unwrap();
    assert_eq!(again.message_id, m.message_id);
    assert!(!again.was_new);
    assert_eq!(store.folders_of(m.message_id).unwrap(), vec![receipts]);
    assert_eq!(
        store.search_threads("moved elsewhere", 50).unwrap().len(),
        1
    );
    assert_eq!(
        store
            .gc(&blobs, T0 + 400 * DAY, 30)
            .unwrap()
            .messages_purged,
        0
    );
}

#[test]
fn a_local_move_never_tombstones() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let m = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("lm@x", "Local move", &[], "archived here"),
        )
        .unwrap();
    let thread = store.thread_of(m.message_id).unwrap().unwrap();
    let receipt = store
        .apply_thread_action(
            a,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    // The drain confirms the move and drops the source placement.
    store.remove_placement(m.message_id, a, "INBOX").unwrap();
    assert_eq!(store.search_threads("archived here", 50).unwrap().len(), 1);
    // And undo, which puts the placement back.
    assert!(store.undo_action(receipt.action_id).unwrap());
    assert_eq!(store.search_threads("archived here", 50).unwrap().len(), 1);
    assert_eq!(
        store
            .list_threads(&ListView::Inbox, 0, 10, Sort::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_complete_relisting_after_a_reset_tombstones_what_it_dropped() {
    let (mut store, blobs, _d) = setup();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(101),
            &mail("alpha@x", "alpha", &[], "alpha body"),
        )
        .unwrap();
    store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(102),
            &mail("beta@x", "beta", &[], "beta body"),
        )
        .unwrap();
    let server = [(1u32, Some("alpha@x".to_string()))];
    let out = store
        .remap_folder_after_reset(inbox, &server, true)
        .unwrap();
    assert_eq!(out.dropped, 1);
    assert_eq!(store.search_threads("alpha body", 50).unwrap().len(), 1);
    assert!(store.search_threads("beta body", 50).unwrap().is_empty());
}

/// The one-off repair: a store carrying ghosts from before the sweep
/// tombstoned anything is mended on open, mirror accounts only.
#[test]
fn opening_an_older_store_repairs_its_ghosts() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let flags = rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC;
        conn.create_scalar_function("petrel_cjk", 1, flags, |ctx| ctx.get::<Option<String>>(0))
            .unwrap();
        conn.create_scalar_function("petrel_has_cjk", 1, flags, |_ctx| Ok(false))
            .unwrap();
        conn.execute_batch(include_str!("../src/store/schema.sql"))
            .unwrap();
        conn.execute(
            "INSERT INTO accounts(id, kind, email, local_archive) VALUES (1, 'imap', 'a@example.com', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO accounts(id, kind, email, local_archive) VALUES (2, 'imap', 'b@example.com', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folders(id, account_id, role, name, path) VALUES (1, 1, 'inbox', 'INBOX', 'INBOX')",
            [],
        )
        .unwrap();
        // 1: placed and live. 2: a ghost in the mirror account. 3: a ghost
        // in the local archive. 4: an index row whose message is gone.
        for (id, account) in [(1, 1), (2, 1), (3, 2)] {
            conn.execute(
                "INSERT INTO messages(id, account_id, date_ms, subject, message_id_hdr)
                 VALUES (?1, ?2, ?3, 'Old subject', ?4)",
                rusqlite::params![id, account, T0 + id, format!("old{id}@x")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
                 VALUES (?1, 'Old subject', 'old body', '', '')",
                [id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             VALUES (4, 'Old subject', 'orphan body', '', '')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO placements(message_id, folder_id, uid) VALUES (1, 1, 1)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let store = Store::open(&db).unwrap();
    store.set_active_account(1).unwrap();
    let hits = store.search_threads("old", 10).unwrap();
    assert_eq!(hits.len(), 1, "the ghost and the orphan are gone: {hits:?}");
    assert_eq!(hits[0].id, 1);
    store.set_active_account(2).unwrap();
    assert_eq!(
        store.search_threads("old", 10).unwrap().len(),
        1,
        "the local archive keeps its unplaced mail"
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    let orphans: i64 = conn
        .query_row(
            "SELECT count(*) FROM fts_content
             WHERE message_id NOT IN (SELECT id FROM messages WHERE deleted_at_ms IS NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
    let tombstoned: Vec<i64> = conn
        .prepare("SELECT id FROM messages WHERE deleted_at_ms IS NOT NULL ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tombstoned, vec![2]);
}
