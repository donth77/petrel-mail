//! A change queued while the account could not reach the server, and the
//! server renumbering that folder meanwhile (fifth audit #168).
//!
//! The person archives, or deletes for good, while offline or signed out.
//! Before Petrel reconnects, another client deletes a message and the server
//! renumbers the folder under a new UIDVALIDITY: a mailbox migrated, an index
//! rebuilt. The drain runs before the sync looks at the folder, so it met the
//! old numbers first, and `UID MOVE 2` archived the message now numbered 2,
//! not the one the person archived. Delete forever erased it. The number is
//! now checked against the UIDVALIDITY it was learned under, the change waits
//! until the folder is mapped again, and then it goes to its own message.

use crate::state::{AppState, test_state};
use crate::sync::drain::drain_actions;
use crate::sync::stateful_imap::{Imap, raw_msg, serve};
use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::flags;
use petrel_providers::imap::{Credential, ImapConfig, Security};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn plain(port: u16) -> ImapConfig {
    ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "u".into(),
        credential: Credential::password("pw"),
        security: Security::InsecurePlaintext,
    }
}

/// The store as the last sync left it: the folders by role, every message
/// the server holds ingested at its UID, and each folder's UIDVALIDITY,
/// UIDNEXT and backfill recorded. Returns the ids of `ids_in`'s messages,
/// in UID order.
fn synced(state: &AppState, srv: &Imap, folders: &[(&str, &str)], ids_in: &str) -> Vec<i64> {
    let account = state.account_id;
    let mut store = state.store.lock().unwrap();
    let rows: Vec<(String, Option<String>)> = folders
        .iter()
        .map(|(path, role)| (path.to_string(), Some(role.to_string())))
        .collect();
    store.sync_folders(account, &rows).unwrap();
    let mut ids = Vec::new();
    for (path, role) in folders {
        let fid = store.folder_for_role(account, role).unwrap().unwrap();
        let held = srv.held(path);
        for m in &held {
            let ingested = store
                .ingest_raw(&state.blobs, account, Some(fid), Some(m.uid), &m.raw)
                .unwrap();
            let seen = m.flags.iter().any(|f| f == "\\Seen");
            store
                .set_message_flags(ingested.message_id, if seen { flags::SEEN } else { 0 })
                .unwrap();
            if *path == ids_in {
                ids.push(ingested.message_id);
            }
        }
        store.set_folder_validity(fid, Some(1)).unwrap();
        let next = held.iter().map(|m| m.uid).max().unwrap_or(0) + 1;
        store.set_folder_uidnext(fid, next).unwrap();
        store.set_backfill_floor(fid, 1).unwrap();
    }
    ids
}

/// Where the store has a message, by folder path; empty when it is gone.
fn where_is(state: &AppState, id: i64) -> Vec<String> {
    let store = state.store.lock().unwrap();
    if store.thread_message(id).ok().flatten().is_none() {
        return Vec::new();
    }
    store
        .folders_of(id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| store.folder_path(f).ok().flatten())
        .collect()
}

fn subjects(srv: &Imap, mailbox: &str) -> Vec<String> {
    srv.subjects(mailbox).into_iter().map(|(_, s)| s).collect()
}

fn queued(state: &AppState) -> usize {
    let account = state.account_id;
    state
        .store
        .lock()
        .unwrap()
        .pending_actions(account)
        .unwrap()
        .len()
}

/// Waits for `done`, up to `limit`: the test's patience, not its verdict.
async fn until(limit: Duration, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() && started.elapsed() < limit {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Mail in a folder: Z, A, B and C at UIDs 1–4, under UIDVALIDITY 1.
fn four_messages(srv: &Imap, mailbox: &str) {
    for s in ["Z", "A", "B", "C"] {
        srv.put(
            mailbox,
            raw_msg(&format!("{s}@example.com"), &format!("msg {s}"), s),
            &["\\Seen"],
        );
    }
}

/// Meanwhile, on the server: another client deletes Z, and the folder is
/// renumbered under UIDVALIDITY 2. A is 1 now, B is 2 and C is 3.
fn renumbered(srv: &Imap, mailbox: &str) {
    srv.expunge_where(mailbox, "msg Z");
    srv.renumber(mailbox, 2);
}

/// Pass 2's probe, as it ran: the real first pass, which drains before it
/// syncs. It archived msg B and left msg A in the inbox.
#[test]
fn a_queued_archive_after_the_folder_is_renumbered_archives_its_own_message() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Archive", "\\Archive")]);
    four_messages(&srv, "INBOX");
    let ids = synced(
        &state,
        &srv,
        &[("INBOX", "inbox"), ("Archive", "archive")],
        "INBOX",
    );
    let (a, b) = (ids[1], ids[2]);
    // Archived while the server could not be reached.
    {
        let store = state.store.lock().unwrap();
        let thread = store.thread_of(a).unwrap().unwrap_or(-a);
        store
            .apply_thread_action(
                account,
                thread,
                ActionKind::Archive,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }
    renumbered(&srv, "INBOX");
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port));
        until(Duration::from_secs(30), || {
            !state.is_seeding(account) && queued(&state) == 0
        })
        .await;
        state.stop_workers(account);
    });
    assert_eq!(
        subjects(&srv, "Archive"),
        ["msg A"],
        "the server archived the wrong message"
    );
    assert_eq!(subjects(&srv, "INBOX"), ["msg B", "msg C"]);
    assert_eq!(where_is(&state, a), ["Archive"]);
    assert_eq!(where_is(&state, b), ["INBOX"]);
}

/// The same with Delete forever, in the Trash: it erased msg B from the
/// server and the store, and msg A came back into the Trash.
#[test]
fn a_queued_delete_forever_after_the_trash_is_renumbered_erases_its_own_message() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Trash", "\\Trash")]);
    for s in ["Z", "A", "B"] {
        srv.put(
            "Trash",
            raw_msg(&format!("{s}@example.com"), &format!("msg {s}"), s),
            &["\\Seen"],
        );
    }
    let ids = synced(
        &state,
        &srv,
        &[("INBOX", "inbox"), ("Trash", "trash")],
        "Trash",
    );
    let (a, b) = (ids[1], ids[2]);
    state
        .store
        .lock()
        .unwrap()
        .apply_message_action(
            account,
            a,
            ActionKind::DeleteForever,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    renumbered(&srv, "Trash");
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port));
        until(Duration::from_secs(30), || {
            !state.is_seeding(account) && queued(&state) == 0
        })
        .await;
        state.stop_workers(account);
    });
    assert_eq!(
        subjects(&srv, "Trash"),
        ["msg B"],
        "the server erased the wrong message"
    );
    assert_eq!(where_is(&state, b), ["Trash"], "msg B is still here");
    // Deleted, not merely out of every folder: the re-map fetched msg A
    // back, and a live row with no folder answers searches and is never
    // collected.
    let gone = state
        .store
        .lock()
        .unwrap()
        .thread_message(a)
        .unwrap()
        .is_none();
    assert!(gone, "msg A stays deleted here");
}

/// What happens in between, step by step: the drain before the sync acts
/// on nothing and keeps the changes queued, uncounted; the sync's re-map
/// learns the numbers again; and the next drain delivers each change to its
/// own message. The archive's number left with the inbox placement, so it is
/// found again by Message-ID; the mark-read's placement is numbered again by
/// the re-map itself.
#[test]
fn a_queued_change_waits_for_the_renumbered_folder_and_then_goes() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Archive", "\\Archive")]);
    four_messages(&srv, "INBOX");
    let ids = synced(
        &state,
        &srv,
        &[("INBOX", "inbox"), ("Archive", "archive")],
        "INBOX",
    );
    let (a, b, c) = (ids[1], ids[2], ids[3]);
    let actions: Vec<i64> = {
        let store = state.store.lock().unwrap();
        // Read, then marked unread here: the server's flag is to come off.
        let thread_c = store.thread_of(c).unwrap().unwrap_or(-c);
        let thread_a = store.thread_of(a).unwrap().unwrap_or(-a);
        vec![
            store
                .apply_thread_action(
                    account,
                    thread_a,
                    ActionKind::Archive,
                    None,
                    PlacementPolicy::Exclusive,
                )
                .unwrap()
                .action_id,
            store
                .apply_thread_action(
                    account,
                    thread_c,
                    ActionKind::MarkUnread,
                    None,
                    PlacementPolicy::Exclusive,
                )
                .unwrap()
                .action_id,
        ]
    };
    renumbered(&srv, "INBOX");
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        let cfg = plain(port);
        let stop = state.stop_signal(account);
        let drain = || {
            drain_actions(
                Arc::clone(&state),
                account,
                cfg.clone(),
                true,
                true,
                false,
                &stop,
            )
        };

        // Before the sync has noticed: nothing moves, nothing is flagged.
        assert!(drain().await);
        let acted: Vec<String> = ["UID MOVE", "UID COPY", "UID STORE", "UID EXPUNGE"]
            .iter()
            .flat_map(|c| srv.said(c))
            .collect();
        assert!(acted.is_empty(), "acted on the old numbers: {acted:?}");
        assert_eq!(subjects(&srv, "INBOX"), ["msg A", "msg B", "msg C"]);
        assert_eq!(queued(&state), 2, "both changes wait");

        // The sync notices the renumbering and maps the folder again.
        super::run_sync_cycle(&state, account, &cfg, true, super::Scope::Everything).await;

        // And now each goes to its own message.
        assert!(drain().await);
    });
    assert_eq!(subjects(&srv, "Archive"), ["msg A"]);
    assert_eq!(subjects(&srv, "INBOX"), ["msg B", "msg C"]);
    let flags: Vec<(String, Vec<String>)> = srv
        .held("INBOX")
        .into_iter()
        .zip(subjects(&srv, "INBOX"))
        .map(|(m, s)| (s, m.flags))
        .collect();
    assert_eq!(
        flags,
        [
            ("msg B".to_string(), vec!["\\Seen".to_string()]),
            ("msg C".to_string(), Vec::new()),
        ],
        "msg C, and only msg C, was marked unread"
    );
    assert_eq!(where_is(&state, a), ["Archive"]);
    assert_eq!(where_is(&state, b), ["INBOX"]);
    assert_eq!(queued(&state), 0, "delivered");
    let store = state.store.lock().unwrap();
    for action in actions {
        assert_eq!(store.action_state(action).unwrap().as_deref(), Some("sent"));
        // Never counted against: waiting for the folder is not a failure.
        assert_eq!(store.record_attempt(action).unwrap(), 1);
    }
}

/// docs/24 #30: the drain's lines name a folder by its id. A path is the
/// person's own words, and sync.log is what gets attached to bug reports.
#[test]
fn the_drain_names_folders_by_id_in_sync_log() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new(
        "MOVE UIDPLUS",
        &[("INBOX", ""), ("Hartwell Estate Matter", "")],
    );
    srv.put(
        "Hartwell Estate Matter",
        raw_msg("h1@example.com", "the will", "h"),
        &["\\Seen"],
    );
    let (filed, source) = {
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Hartwell Estate Matter".into(), None),
                ],
            )
            .unwrap();
        let source = store
            .ensure_named_folder(account, "Hartwell Estate Matter")
            .unwrap();
        // Its number lost, as a renumbering leaves it: the drain asks the
        // folder for it by Message-ID.
        let raw = srv.held("Hartwell Estate Matter")[0].raw.clone();
        let ingested = store
            .ingest_raw(&state.blobs, account, Some(source), None, &raw)
            .unwrap();
        (ingested.message_id, source)
    };
    // Filed into a folder made here that the server does not have yet: the
    // move is refused and the folder made before it is tried again.
    {
        let store = state.store.lock().unwrap();
        let target = store
            .ensure_named_folder(account, "Brightwater Contracts")
            .unwrap();
        let thread = store.thread_of(filed).unwrap().unwrap_or(-filed);
        store
            .apply_thread_action(
                account,
                thread,
                ActionKind::Move,
                Some(target),
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        let stop = state.stop_signal(account);
        assert!(
            drain_actions(
                Arc::clone(&state),
                account,
                plain(port),
                true,
                true,
                false,
                &stop
            )
            .await
        );
    });
    assert_eq!(subjects(&srv, "Brightwater Contracts"), ["the will"]);
    let log = crate::diag::test_log_text();
    let leaks: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("Hartwell") || l.contains("Brightwater"))
        .collect();
    assert!(leaks.is_empty(), "folder names in sync.log: {leaks:#?}");
    assert!(
        log.contains(&format!("folder {source} answers to the Message-ID")),
        "and the line is still there, by id"
    );
}

/// The same root in the removal of a sent draft's server copies, which waits
/// while the account is signed out: the copy's number was read when the
/// removal was held, and Drafts renumbered meanwhile gave it to another
/// draft. Nothing is erased under the old numbering; once the sync has mapped
/// the folder again, the copy the push recorded follows its placement, and
/// the removal finds the right one.
#[test]
fn a_held_removal_after_drafts_is_renumbered_erases_no_other_draft() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
    srv.put(
        "Drafts",
        raw_msg("w@example.com", "another draft, W", "w"),
        &[],
    );
    srv.put(
        "Drafts",
        raw_msg("sent-draft@example.com", "Plans", "yes"),
        &[],
    );
    srv.put(
        "Drafts",
        raw_msg("x@example.com", "another draft, X", "x"),
        &[],
    );
    // The draft as its last push left it: the copy at UID 2 in Drafts, under
    // UIDVALIDITY 1, recorded and numbered.
    let draft = {
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Drafts".into(), Some("drafts".into())),
                ],
            )
            .unwrap();
        let drafts = store.folder_for_role(account, "drafts").unwrap().unwrap();
        store.set_folder_validity(drafts, Some(1)).unwrap();
        store.set_folder_uidnext(drafts, 4).unwrap();
        store.set_backfill_floor(drafts, 1).unwrap();
        let id = store
            .save_draft(account, None, "dana@example.com", "Plans", "yes", "")
            .unwrap();
        store.set_draft_msgid(id, "sent-draft@example.com").unwrap();
        store.set_draft_server_uid(id, Some(2)).unwrap();
        store
            .heal_placement_uid(id, account, "Drafts", 2, Some(1))
            .unwrap();
        id
    };
    // Held while signed out: read now, sent later.
    let held = {
        let store = state.store.lock().unwrap();
        crate::sync::drafts::server_copies(&store, account, draft)
    };
    // Meanwhile W is deleted elsewhere and Drafts renumbered: the sent
    // draft's copy is 1, and X is 2.
    srv.expunge_where("Drafts", "another draft, W");
    srv.renumber("Drafts", 2);
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        let cfg = plain(port);
        crate::sync::drafts::expunge_copies(&cfg, held, true).await;
        assert_eq!(
            subjects(&srv, "Drafts"),
            ["Plans", "another draft, X"],
            "another draft was erased by the old number"
        );
        super::run_sync_cycle(&state, account, &cfg, true, super::Scope::Everything).await;
        let again = {
            let store = state.store.lock().unwrap();
            crate::sync::drafts::server_copies(&store, account, draft)
        };
        crate::sync::drafts::expunge_copies(&cfg, again, true).await;
    });
    assert_eq!(subjects(&srv, "Drafts"), ["another draft, X"]);
}

/// Archives `id`'s conversation as a build from before validities were kept
/// queued it: its payload carries no `source_validity`. The store here is in
/// memory, so the row is made the way such a row reads: queued while the
/// folder's validity is NULL, which writes `"source_validity":null`, and
/// serde reads that as it reads a missing key (`#[serde(default)]`).
fn archive_queued_before_the_upgrade(state: &AppState, id: i64) {
    let account = state.account_id;
    let mut store = state.store.lock().unwrap();
    let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
    store.set_folder_validity(inbox, None).unwrap();
    let thread = store.thread_of(id).unwrap().unwrap_or(-id);
    store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    store.set_folder_validity(inbox, Some(1)).unwrap();
}

/// Review r1, R1-1: the same archive, queued before the upgrade, so its
/// number came with no UIDVALIDITY. It went with the folder's validity as it
/// stood: refused while that was the old one, and once the sync had mapped
/// the folder again and recorded the new one, sent under it. `UID MOVE 2`
/// then archived msg B, which is #168 again. It takes a message the re-map
/// does not place again, or the inbox row the re-map makes wins: here msg A
/// is older than the newest 200 it lists, as in any ordinary inbox. A number
/// with no validity is now asked for by Message-ID, wherever the folder has
/// one to check it against.
#[test]
fn an_archive_queued_before_the_upgrade_archives_its_own_message_after_the_remap() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Archive", "\\Archive")]);
    four_messages(&srv, "INBOX");
    for i in 0..201 {
        srv.put(
            "INBOX",
            raw_msg(&format!("n{i}@example.com"), &format!("newer {i}"), "n"),
            &["\\Seen"],
        );
    }
    let ids = synced(
        &state,
        &srv,
        &[("INBOX", "inbox"), ("Archive", "archive")],
        "INBOX",
    );
    archive_queued_before_the_upgrade(&state, ids[1]);
    renumbered(&srv, "INBOX");
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        crate::sync::spawn_real_sync(Arc::clone(&state), account, plain(port));
        until(Duration::from_secs(40), || {
            !state.is_seeding(account) && queued(&state) == 0
        })
        .await;
        // The drain that recovery nudges goes 900 ms later; let it.
        tokio::time::sleep(Duration::from_secs(2)).await;
        state.stop_workers(account);
    });
    let moves = srv.said("UID MOVE");
    assert_eq!(
        subjects(&srv, "Archive"),
        ["msg A"],
        "the archive went to another message; MOVEs sent: {moves:?}"
    );
}

/// Review r1, R1-2: a folder renumbered under a long queue. Every change in
/// it is refused until the sync maps it again, and a refused flag batch left
/// its rows unmarked, so each row began a batch of its own on a connection of
/// its own: 120 queued mark-reads were 120 sign-ins in one pass, and the
/// first pass's sync, the only thing that releases them, waited behind all
/// of them. The first refusal now stands for the folder's other rows, for
/// the rest of the pass.
#[test]
fn a_renumbered_folder_costs_one_sign_in_however_many_changes_wait_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", "")]);
    const K: usize = 120;
    for i in 0..K {
        srv.put(
            "INBOX",
            raw_msg(&format!("u{i}@example.com"), &format!("unread {i}"), "u"),
            &[],
        );
    }
    let ids = synced(&state, &srv, &[("INBOX", "inbox")], "INBOX");
    {
        let store = state.store.lock().unwrap();
        for id in &ids {
            let thread = store.thread_of(*id).unwrap().unwrap_or(-id);
            store
                .apply_thread_action(
                    account,
                    thread,
                    ActionKind::MarkRead,
                    None,
                    PlacementPolicy::Exclusive,
                )
                .unwrap();
        }
    }
    assert_eq!(queued(&state), K);
    // Renumbered with nothing removed: the same order, a new validity.
    srv.renumber("INBOX", 2);
    let logins = tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        let stop = state.stop_signal(account);
        assert!(
            drain_actions(
                Arc::clone(&state),
                account,
                plain(port),
                true,
                true,
                false,
                &stop
            )
            .await
        );
        srv.said("LOGIN").len()
    });
    assert_eq!(queued(&state), K, "every change waits, as it should");
    assert!(srv.said("UID STORE").is_empty(), "nothing was flagged");
    assert_eq!(
        logins, 1,
        "sign-ins in one drain pass for {K} changes queued in one renumbered folder"
    );
}

/// The same in Empty Trash and the bin's expiry, which expunge one message
/// per connection: a renumbered Trash of 30 was 30 sign-ins, each refused.
#[test]
fn emptying_a_renumbered_trash_signs_in_once() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Trash", "\\Trash")]);
    const K: usize = 30;
    for i in 0..K {
        srv.put(
            "Trash",
            raw_msg(&format!("t{i}@example.com"), &format!("binned {i}"), "t"),
            &["\\Seen"],
        );
    }
    synced(
        &state,
        &srv,
        &[("INBOX", "inbox"), ("Trash", "trash")],
        "Trash",
    );
    srv.renumber("Trash", 2);
    let items = {
        let store = state.store.lock().unwrap();
        crate::commands::triage::binned(&store, account, store.trash_contents(account).unwrap())
    };
    let (outcome, logins) = tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        let outcome = crate::commands::triage::expunge_binned(
            &state,
            account,
            Some(plain(port)),
            true,
            items,
        )
        .await;
        (outcome, srv.said("LOGIN").len())
    });
    assert_eq!(
        outcome,
        Ok((0, K)),
        "each one is kept until the Trash is mapped again"
    );
    assert_eq!(subjects(&srv, "Trash").len(), K);
    assert_eq!(
        logins, 1,
        "sign-ins for one Empty Trash of a renumbered Trash"
    );
}

/// Review r1, R1-5: one row stands for every copy of one message. Sent to
/// yourself and binned, a message is in Sent and the Trash, and Delete
/// forever is one action with a row for each copy. The first expunge settles
/// it, and the drain then made the row a tombstone again by dropping every
/// placement it had, the other copy's too: Sent held one message fewer than
/// the server, the reconcile fetched the copy it was "missing", and the
/// message deleted forever was back. Only the expunged copy's placement goes.
#[test]
fn a_message_deleted_forever_stays_deleted_after_the_reconcile() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new(
        "MOVE UIDPLUS",
        &[("INBOX", ""), ("Trash", "\\Trash"), ("Sent", "\\Sent")],
    );
    let copy = raw_msg("selfsent@example.com", "note to self", "m");
    srv.put("Trash", copy.clone(), &["\\Seen"]);
    srv.put(
        "Trash",
        raw_msg("t1@example.com", "binned", "t"),
        &["\\Seen"],
    );
    srv.put("Sent", copy, &["\\Seen"]);
    srv.put("Sent", raw_msg("s1@example.com", "sent", "s"), &["\\Seen"]);
    let ids = synced(
        &state,
        &srv,
        &[("INBOX", "inbox"), ("Trash", "trash"), ("Sent", "sent")],
        "Trash",
    );
    let note = ids[0];
    {
        let store = state.store.lock().unwrap();
        assert_eq!(
            store.folders_of(note).unwrap().len(),
            2,
            "one row, two copies"
        );
        store
            .apply_message_action(
                account,
                note,
                ActionKind::DeleteForever,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
    }
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        let cfg = plain(port);
        let stop = state.stop_signal(account);
        assert!(
            drain_actions(
                Arc::clone(&state),
                account,
                cfg.clone(),
                true,
                true,
                false,
                &stop
            )
            .await
        );
        super::reconcile_ghost_placements(&state, account, &cfg).await;
    });
    let back = state
        .store
        .lock()
        .unwrap()
        .message_by_msgid(account, "selfsent@example.com")
        .unwrap();
    let left = (subjects(&srv, "Trash"), subjects(&srv, "Sent"));
    assert_eq!(
        back, None,
        "the message deleted forever is live again; the server holds {left:?}"
    );
}

/// Review r1, R1-7: settling a draft conflict expunges one revision by its
/// number in Drafts, and asked with no UIDVALIDITY. With Drafts renumbered
/// before the sync noticed, "Keep this version" erased whichever draft held
/// the other revision's old number, and "Take the server's" erased the very
/// revision it had taken. The number goes with the validity it was read
/// under now, and the server refuses it: nothing is erased.
#[test]
fn settling_a_draft_conflict_after_drafts_is_renumbered_erases_no_other_draft() {
    for take_server in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        let theirs = raw_msg("d-1@example.com", "Plans, revised", "yes, at six");
        srv.put(
            "Drafts",
            raw_msg("w@example.com", "another draft, W", "w"),
            &[],
        );
        srv.put("Drafts", raw_msg("d-1@example.com", "Plans", "yes"), &[]);
        srv.put("Drafts", theirs.clone(), &[]);
        srv.put(
            "Drafts",
            raw_msg("x@example.com", "another draft, X", "x"),
            &[],
        );
        // The draft with its copy at UID 2, and another client's revision
        // of it at 3, under UIDVALIDITY 1.
        let (draft, other) = {
            let mut store = state.store.lock().unwrap();
            store
                .sync_folders(
                    account,
                    &[
                        ("INBOX".into(), Some("inbox".into())),
                        ("Drafts".into(), Some("drafts".into())),
                    ],
                )
                .unwrap();
            let drafts = store.folder_for_role(account, "drafts").unwrap().unwrap();
            store.set_folder_validity(drafts, Some(1)).unwrap();
            let id = store
                .save_draft(account, None, "dana@example.com", "Plans", "yes", "")
                .unwrap();
            store.set_draft_msgid(id, "d-1@example.com").unwrap();
            store.set_draft_server_uid(id, Some(2)).unwrap();
            store
                .heal_placement_uid(id, account, "Drafts", 2, Some(1))
                .unwrap();
            let other = store
                .ingest_raw_second_copy(&state.blobs, account, Some(drafts), 3, &theirs)
                .unwrap()
                .message_id;
            assert_eq!(store.draft_conflict(id).unwrap(), Some((other, Some(3))));
            (id, other)
        };
        // Meanwhile W is deleted elsewhere and Drafts renumbered: our copy
        // is 1 now, the revision 2, and X 3.
        srv.expunge_where("Drafts", "another draft, W");
        srv.renumber("Drafts", 2);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            crate::commands::compose::settle_draft_conflict(
                &state,
                draft,
                other,
                take_server,
                |_, _| Some(plain(port)),
            )
            .await
            .unwrap();
        });
        assert_eq!(
            subjects(&srv, "Drafts"),
            ["Plans", "Plans, revised", "another draft, X"],
            "erased by a number from before the renumbering (took the server's: {take_server})"
        );
    }
}

/// The gap filler compares the server's UIDs with the store's, and ran before
/// the cycle that notices a renumbering. Under the new numbering every stored
/// UID that was not reused looked gone: D, still on the server, lost its
/// placement and dropped out of the inbox and out of search. The sweep now
/// checks the numbering first and leaves a changed one to the recovery.
#[test]
fn the_gap_filler_leaves_a_renumbered_folder_to_the_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let srv = Imap::new("MOVE UIDPLUS", &[("INBOX", "")]);
    for s in ["Z", "A", "B", "C", "D"] {
        srv.put(
            "INBOX",
            raw_msg(&format!("{s}@example.com"), &format!("msg {s}"), s),
            &[],
        );
    }
    // Sparse numbers, as a folder that has seen deletions has: A at 2, C at
    // 4, D at 5.
    srv.expunge_where("INBOX", "msg Z");
    srv.expunge_where("INBOX", "msg B");
    let ids = synced(&state, &srv, &[("INBOX", "inbox")], "INBOX");
    assert_eq!(ids.len(), 3);
    let d = ids[2];
    // Another client deletes C, and the server renumbers: A is 1, D is 2.
    srv.expunge_where("INBOX", "msg C");
    srv.renumber("INBOX", 2);
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        super::reconcile_ghost_placements(&state, state.account_id, &plain(port)).await;
    });
    assert_eq!(
        where_is(&state, d),
        ["INBOX"],
        "D is on the server; only its number changed"
    );
    assert!(
        srv.said("FETCH").is_empty(),
        "nothing fetched against the old numbers: {:?}",
        srv.said("FETCH")
    );
}

/// A message to the address the test drafts are written to.
fn letter(id: &str, subject: &str, body: &str, date: &str) -> Vec<u8> {
    letter_to(id, "dana@example.com", subject, body, date)
}

fn letter_to(id: &str, to: &str, subject: &str, body: &str, date: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\nTo: {to}\r\n\
         Subject: {subject}\r\nDate: {date}\r\nMessage-ID: <{id}>\r\n\
         MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

/// The reconcile files a second UID of one Message-ID as its own row. An
/// open draft is the exception when that UID only repeats what the draft
/// says: it is this client's push, not another client's revision. A
/// different body is still the revision, and so are the same words to
/// other people.
#[test]
fn an_open_drafts_own_copy_is_not_filed_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("UIDPLUS", &[("Drafts", "\\Drafts")]);
    let id = "d-echo@example.com";
    srv.put(
        "Drafts",
        letter(id, "Plans", "yes", "Tue, 01 Oct 2026 09:00:00 +0000"),
        &[],
    );
    srv.put(
        "Drafts",
        letter(id, "Plans", "yes", "Tue, 01 Oct 2026 09:05:00 +0000"),
        &[],
    );
    srv.put(
        "Drafts",
        letter(id, "Plans", "their edit", "Tue, 01 Oct 2026 09:06:00 +0000"),
        &[],
    );
    srv.put(
        "Drafts",
        letter_to(
            id,
            "dana@example.com, sam@example.com",
            "Plans",
            "yes",
            "Tue, 01 Oct 2026 09:07:00 +0000",
        ),
        &[],
    );
    let (draft, folder) = {
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(account, &[("Drafts".into(), Some("drafts".into()))])
            .unwrap();
        let folder = store.folder_for_role(account, "drafts").unwrap().unwrap();
        let draft = store
            .save_draft(account, None, "dana@example.com", "Plans", "yes", "")
            .unwrap();
        store.set_draft_msgid(draft, id).unwrap();
        store.set_folder_validity(folder, Some(1)).unwrap();
        assert!(
            store
                .heal_placement_uid(draft, account, "Drafts", 1, Some(1))
                .unwrap()
        );
        store.set_draft_server_uid(draft, Some(1)).unwrap();
        store.set_backfill_floor(folder, 1).unwrap();
        (draft, folder)
    };
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        super::reconcile_ghost_placements(&state, account, &plain(port)).await;
    });
    let mut uids = state.store.lock().unwrap().placement_uids(folder).unwrap();
    uids.sort_unstable();
    assert_eq!(
        uids,
        vec![1, 3, 4],
        "the same words became a second draft, or another revision did not"
    );
    assert!(
        state
            .store
            .lock()
            .unwrap()
            .draft_conflict(draft)
            .unwrap()
            .is_some(),
        "the other revision is not offered as a conflict"
    );
}

/// The exception is the open draft. Two deliveries of an ordinary message
/// stay two rows, even when the words match and only the date differs.
#[test]
fn a_second_copy_of_an_ordinary_message_stays_its_own_row() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let account = state.account_id;
    let srv = Imap::new("UIDPLUS", &[("INBOX", "")]);
    let first = letter(
        "m@example.com",
        "Note",
        "hello",
        "Tue, 01 Oct 2026 09:00:00 +0000",
    );
    srv.put("INBOX", first.clone(), &["\\Seen"]);
    srv.put(
        "INBOX",
        letter(
            "m@example.com",
            "Note",
            "hello",
            "Tue, 01 Oct 2026 09:05:00 +0000",
        ),
        &["\\Seen"],
    );
    let folder = {
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(account, &[("INBOX".into(), Some("inbox".into()))])
            .unwrap();
        let folder = store.folder_for_role(account, "inbox").unwrap().unwrap();
        store
            .ingest_raw(&state.blobs, account, Some(folder), Some(1), &first)
            .unwrap();
        store.set_folder_validity(folder, Some(1)).unwrap();
        store.set_backfill_floor(folder, 1).unwrap();
        folder
    };
    tauri::async_runtime::block_on(async {
        let port = serve(Arc::clone(&srv)).await;
        super::reconcile_ghost_placements(&state, account, &plain(port)).await;
    });
    let mut uids = state.store.lock().unwrap().placement_uids(folder).unwrap();
    uids.sort_unstable();
    assert_eq!(
        uids,
        vec![1, 2],
        "the second delivery was folded into the first"
    );
}
