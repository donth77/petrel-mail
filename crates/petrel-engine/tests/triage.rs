//! Triage actions apply locally at once and are undoable exactly.
//!
//! The property under test throughout is that undo restores *what was there*,
//! not an inferred inverse — the difference only shows up once more than one
//! action has touched the same message, which is where guessing goes wrong.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{ListView, NewMessage, Store, flags};

fn seeded() -> (Store, i64, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = (0..3)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 1_000 + i,
            from_addr: "a@example.com".into(),
            from_display: "A".into(),
            to_addr: "me@example.com".into(),
            subject: format!("m{i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    (store, account, ids)
}

fn thread_of(store: &Store, id: i64) -> i64 {
    store.thread_of(id).unwrap().unwrap_or(-id)
}

#[test]
fn archiving_moves_the_conversation_and_undo_puts_it_back() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    let tid = thread_of(&store, ids[0]);

    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(receipt.description, "Archived");

    let archive = store.folder_for_role(account, "archive").unwrap().unwrap();
    assert_eq!(
        store.folders_of(ids[0]).unwrap(),
        vec![archive],
        "archived mail leaves every folder it was in and lands in exactly one"
    );

    assert!(store.undo_action(receipt.action_id).unwrap());
    assert_eq!(
        store.folders_of(ids[0]).unwrap(),
        vec![inbox],
        "undo restores the folder it actually came from"
    );
}

#[test]
fn undo_restores_prior_flags_rather_than_inverting_the_action() {
    let (store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);

    // Already read *and* starred before anything happens.
    store
        .set_flags(ids[0], flags::SEEN | flags::FLAGGED, 0)
        .unwrap();

    let r = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::MarkUnread,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let after: i64 = store.flags_of(ids[0]).unwrap();
    assert_eq!(after & flags::SEEN, 0, "marked unread");
    assert_ne!(after & flags::FLAGGED, 0, "and still starred");

    store.undo_action(r.action_id).unwrap();
    let back = store.flags_of(ids[0]).unwrap();
    assert_ne!(back & flags::SEEN, 0, "read state restored");
    assert_ne!(back & flags::FLAGGED, 0, "star untouched by the undo");
}

#[test]
fn an_action_applies_to_every_message_in_the_conversation() {
    let (store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);
    let r = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Star,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    // These are unthreaded bulk inserts, so each is its own conversation — the
    // count is what the receipt must report, whatever the threading produced.
    assert_eq!(r.message_count, 1);
    assert_ne!(store.flags_of(ids[0]).unwrap() & flags::FLAGGED, 0);
}

#[test]
fn undo_is_refused_once_the_action_has_left_the_queue() {
    let (store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);
    let r = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Star,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    store.mark_action_state(r.action_id, "sent").unwrap();

    assert!(
        !store.undo_action(r.action_id).unwrap(),
        "once the server knows, undoing is a new action — not a cancellation"
    );
    assert_ne!(
        store.flags_of(ids[0]).unwrap() & flags::FLAGGED,
        0,
        "and nothing is silently rolled back behind the user"
    );
}

#[test]
fn undoing_twice_is_harmless() {
    let (store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);
    let r = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Star,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    assert!(store.undo_action(r.action_id).unwrap());
    assert!(
        !store.undo_action(r.action_id).unwrap(),
        "a second undo reports that it did nothing rather than re-applying"
    );
    assert_eq!(store.flags_of(ids[0]).unwrap() & flags::FLAGGED, 0);
}

/// The list has to agree with the action. Archiving that moves a message in the
/// store but leaves it in the inbox listing is not archiving — the row comes
/// back on the next refetch, or at the latest when the app restarts.
#[test]
fn archived_conversations_leave_the_listing_and_come_back_on_undo() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    let tid = thread_of(&store, ids[0]);

    let before = store
        .list_threads(
            &ListView::Inbox,
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert_eq!(before.len(), 3, "all three start in the listing");

    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let after = store
        .list_threads(
            &ListView::Inbox,
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert_eq!(after.len(), 2, "the archived conversation left the listing");
    assert!(
        !after.iter().any(|t| t.thread_id == tid),
        "archived conversation must not be listed"
    );

    store.undo_action(receipt.action_id).unwrap();
    let restored = store
        .list_threads(
            &ListView::Inbox,
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert_eq!(restored.len(), 3, "undo puts it back in the listing");
    assert!(restored.iter().any(|t| t.thread_id == tid));
}

/// The reversal of an older guard, deliberately. The inbox used to mean
/// "not filed anywhere else", and a test here defended unplaced mail's right
/// to appear — sync could once ingest without placing. Both facts changed:
/// every ingest path now places atomically, and on Gmail every message is
/// in All Mail (the archive role), so the moment All Mail synced, "not
/// filed elsewhere" was false of the entire mailbox and the inbox emptied
/// itself. The inbox now means membership — and mail placed nowhere (a
/// draft, a bulk-inserted fixture) is in nobody's inbox.
#[test]
fn unplaced_messages_do_not_haunt_the_inbox() {
    let (store, _account, _ids) = seeded();
    assert_eq!(
        store
            .list_threads(
                &ListView::Inbox,
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .len(),
        0,
        "no placement, no inbox: membership is the meaning now"
    );
}

/// Trash and spam leave the listing for the same reason archive does.
#[test]
fn trashed_and_spammed_conversations_leave_the_listing() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::Trash,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[1]),
            ActionKind::Spam,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(
        store
            .list_threads(
                &ListView::Inbox,
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .len(),
        1
    );
}

/// Moving into a folder the user named, and putting it back exactly.
#[test]
fn moving_to_a_named_folder_is_undoable() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    let dest = store
        .ensure_named_folder(account, "Contracts/2026")
        .unwrap();
    let tid = thread_of(&store, ids[0]);

    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Move,
            Some(dest),
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(store.folders_of(ids[0]).unwrap(), vec![dest]);
    // A named folder is not one of the filed-away roles, so the conversation
    // stays out of the inbox listing but is not in archive/trash/spam either.
    assert!(
        !store
            .list_threads(
                &ListView::Folder("archive".into()),
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .iter()
            .any(|t| t.thread_id == tid)
    );

    store.undo_action(receipt.action_id).unwrap();
    assert_eq!(store.folders_of(ids[0]).unwrap(), vec![inbox]);
}

#[test]
fn a_move_without_a_destination_is_refused_rather_than_guessed() {
    let (store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);
    let err = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Move,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap_err();
    assert!(err.to_string().contains("needs a target"), "{err}");
    // Nothing was queued, so there is nothing to sync and nothing to undo.
    assert!(!store.undo_action(1).unwrap());
}

#[test]
fn ensure_named_folder_is_idempotent_and_case_insensitive() {
    let (store, account, _ids) = seeded();
    let a = store.ensure_named_folder(account, "Receipts").unwrap();
    let b = store.ensure_named_folder(account, "receipts").unwrap();
    assert_eq!(a, b, "one folder, not two that differ only in case");
    assert!(store.ensure_named_folder(account, "   ").is_err());
}

/// Undo restores the tags that were there, rather than removing the one the
/// action added — the two differ once a tag is applied twice.
#[test]
fn undoing_a_tag_restores_prior_tags_rather_than_inverting() {
    let (store, account, ids) = seeded();
    let urgent = store.ensure_tag(account, "urgent", None).unwrap();
    let tid = thread_of(&store, ids[0]);

    // Already tagged. Tagging again must leave it tagged after an undo.
    store.tag_message(ids[0], urgent).unwrap();
    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Tag,
            Some(urgent),
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    store.undo_action(receipt.action_id).unwrap();
    assert_eq!(
        store.tags_of(ids[0]).unwrap(),
        vec![urgent],
        "undo put back what was there; inverting would have removed it"
    );
}

#[test]
fn untagging_and_undoing_it_puts_the_tag_back() {
    let (store, account, ids) = seeded();
    let urgent = store.ensure_tag(account, "urgent", None).unwrap();
    let tid = thread_of(&store, ids[0]);
    store.tag_message(ids[0], urgent).unwrap();

    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Untag,
            Some(urgent),
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert!(store.tags_of(ids[0]).unwrap().is_empty());
    store.undo_action(receipt.action_id).unwrap();
    assert_eq!(store.tags_of(ids[0]).unwrap(), vec![urgent]);
}

/// The bug this exists to prevent: on a labels provider, archiving must not
/// throw away the folders a user deliberately filed a conversation under.
#[test]
fn archiving_on_a_labels_provider_keeps_other_labels() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let work = store.ensure_named_folder(account, "Work").unwrap();
    store.place_message(ids[0], inbox).unwrap();
    store.place_message(ids[0], work).unwrap();

    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::Archive,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();

    let after = store.folders_of(ids[0]).unwrap();
    assert!(!after.contains(&inbox), "it left the inbox");
    assert!(
        after.contains(&work),
        "Work must survive: the user put it there"
    );
}

/// ...and on a classic server, where a message lives in exactly one folder,
/// archiving really does clear what was there.
#[test]
fn archiving_on_an_exclusive_provider_replaces_the_placement() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    store.place_message(ids[0], inbox).unwrap();

    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    let after = store.folders_of(ids[0]).unwrap();
    assert!(!after.contains(&inbox));
    assert_eq!(after.len(), 1, "exactly one folder, the archive");
}

/// Binning is exclusive whatever the provider: a trashed message is not still
/// sitting in the inbox under a label.
#[test]
fn trashing_clears_labels_even_on_a_labels_provider() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let work = store.ensure_named_folder(account, "Work").unwrap();
    store.place_message(ids[0], inbox).unwrap();
    store.place_message(ids[0], work).unwrap();

    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::Trash,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();

    assert_eq!(store.folders_of(ids[0]).unwrap().len(), 1);
}

/// Undo puts back every label, not just the inbox one it removed.
#[test]
fn undoing_a_labels_archive_restores_the_full_label_set() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let work = store.ensure_named_folder(account, "Work").unwrap();
    store.place_message(ids[0], inbox).unwrap();
    store.place_message(ids[0], work).unwrap();

    let r = store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::Archive,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();
    store.undo_action(r.action_id).unwrap();

    let mut after = store.folders_of(ids[0]).unwrap();
    after.sort();
    let mut want = vec![inbox, work];
    want.sort();
    assert_eq!(after, want);
}

/// A resync must not undo work the server has not seen yet.
#[test]
fn pending_local_changes_survive_a_resync() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }

    // The user marks it read locally; the action is queued, not yet delivered.
    let receipt = store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert!(store.message_has_pending(ids[0]).unwrap());
    assert_eq!(store.flags_of(ids[0]).unwrap() & flags::SEEN, flags::SEEN);

    // A resync now reports the server's stale view: still unread.
    store.set_message_flags(ids[0], 0).unwrap();
    assert_eq!(
        store.flags_of(ids[0]).unwrap() & flags::SEEN,
        flags::SEEN,
        "the queued change wins until it has been delivered"
    );

    // Once delivered, the server is authoritative again.
    store.mark_action_state(receipt.action_id, "sent").unwrap();
    assert!(!store.message_has_pending(ids[0]).unwrap());
    store.set_message_flags(ids[0], 0).unwrap();
    assert_eq!(store.flags_of(ids[0]).unwrap() & flags::SEEN, 0);
}

/// Untouched messages are not protected — the guard is per message, not a
/// blanket freeze on the mailbox while anything is queued.
#[test]
fn a_pending_action_does_not_freeze_every_other_message() {
    let (store, account, ids) = seeded();
    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    assert!(!store.message_has_pending(ids[1]).unwrap());
    store.set_message_flags(ids[1], flags::SEEN).unwrap();
    assert_eq!(store.flags_of(ids[1]).unwrap() & flags::SEEN, flags::SEEN);
}

/// The drain needs actions oldest-first, or two changes to the same message
/// arrive in the wrong order and the later one loses.
#[test]
fn pending_actions_come_back_in_the_order_they_were_made() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    store.place_message(ids[0], inbox).unwrap();
    let tid = thread_of(&store, ids[0]);

    let first = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Star,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let second = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Unstar,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    let queued = store.pending_actions(account).unwrap();
    let order: Vec<i64> = queued.iter().map(|p| p.action_id).collect();
    assert_eq!(order, vec![first.action_id, second.action_id]);
    // And each knows how to address the message on the server.
    assert!(queued.iter().all(|p| p.folder_path == "INBOX"));
}

/// A mark_read of an already-read conversation must not queue a no-op, or a
/// 22k-message thread you merely reopen rebuilds a multi-megabyte undo snapshot.
#[test]
fn mark_read_on_an_already_read_thread_queues_nothing() {
    let (store, account, ids) = seeded();
    store.set_flags(ids[0], flags::SEEN, 0).unwrap();
    let receipt = store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(receipt.action_id, 0);
    assert_eq!(receipt.message_count, 0);
    assert!(store.pending_actions(account).unwrap().is_empty());
    assert_eq!(store.flags_of(ids[0]).unwrap() & flags::SEEN, flags::SEEN);
}

/// Only the messages whose SEEN bit actually changes go on the queue.
#[test]
fn mark_read_skips_messages_already_seen() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let ids = ingest_reply_chain(&mut store, &blobs, account, inbox, 3);
    store.set_flags(ids[0], flags::SEEN, 0).unwrap();
    let thread = store.thread_of(ids[0]).unwrap().unwrap();

    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(receipt.message_count, 2);
    let pending = store.pending_actions(account).unwrap();
    let queued: Vec<i64> = pending.iter().map(|p| p.message_id).collect();
    assert!(!queued.contains(&ids[0]));
    assert!(queued.contains(&ids[1]));
    assert!(queued.contains(&ids[2]));
}

/// Drain used to clone the undo JSON onto every action_messages row.
#[test]
fn pending_actions_share_one_payload_per_action() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let ids = ingest_reply_chain(&mut store, &blobs, account, inbox, 8);
    let thread = store.thread_of(ids[0]).unwrap().unwrap();
    store
        .apply_thread_action(
            account,
            thread,
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    let pending = store.pending_actions(account).unwrap();
    assert_eq!(pending.len(), 8);
    let first = &pending[0].payload_json;
    assert!(
        pending
            .iter()
            .all(|p| std::sync::Arc::ptr_eq(&p.payload_json, first)),
        "one undo snapshot, not one copy per message"
    );
}

fn ingest_reply_chain(
    store: &mut Store,
    blobs: &petrel_engine::blob::BlobStore,
    account: i64,
    inbox: i64,
    n: usize,
) -> Vec<i64> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let reply = if i == 0 {
            String::new()
        } else {
            format!("In-Reply-To: <m{}@x>\r\nReferences: <m0@x>\r\n", i - 1)
        };
        let raw = format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: shared chain\r\n\
             Message-ID: <m{i}@x>\r\n{reply}\
             MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n"
        );
        let m = store
            .ingest_raw(
                blobs,
                account,
                Some(inbox),
                Some(100 + i as u32),
                raw.as_bytes(),
            )
            .unwrap();
        ids.push(m.message_id);
    }
    let thread = store.thread_of(ids[0]).unwrap().unwrap();
    for id in &ids {
        assert_eq!(
            store.thread_of(*id).unwrap().unwrap(),
            thread,
            "reply chain must be one conversation"
        );
    }
    ids
}

/// Permanent delete is the one action with no way back, and every layer has to
/// agree about that — a client that offers undo and then cannot deliver it is
/// worse than one that never offered.
#[test]
fn deleting_forever_leaves_the_list_and_cannot_be_undone() {
    let (store, account, ids) = seeded();
    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    store.place_message(ids[0], trash).unwrap();
    let thread = store.thread_of(ids[0]).unwrap().unwrap_or(-ids[0]);

    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::DeleteForever,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    // Gone from the view it was in, not merely moved somewhere else.
    let left: Vec<i64> = store
        .list_threads(
            &ListView::Folder("trash".into()),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(!left.contains(&ids[0]), "a deleted message still lists");

    // And it still needs delivering, or the server would keep handing it back.
    assert!(
        !ActionKind::DeleteForever.is_local_only(),
        "the expunge has to reach the server"
    );
    assert!(!store.undo_action(receipt.action_id).unwrap());

    // Refusing undo must not have half-restored it on the way out.
    let after: Vec<i64> = store
        .list_threads(
            &ListView::Folder("trash".into()),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(!after.contains(&ids[0]));
}

/// The move that could never be delivered.
mod move_delivery {
    use petrel_engine::actions::ActionKind;
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::Store;

    #[test]
    fn a_move_keeps_its_server_address_after_destroying_its_placement() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let dest = store.ensure_named_folder(account, "Projects").unwrap();
        let raw = b"From: a@example.com\r\nTo: b@example.com\r\nSubject: s\r\n\
Message-ID: <m1@x>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n";
        let m = store
            .ingest_raw(&blobs, account, Some(inbox), Some(77), raw)
            .unwrap();
        let thread = store.thread_of(m.message_id).unwrap().unwrap();
        let policy = store.placement_policy(account).unwrap();

        store
            .apply_thread_action(account, thread, ActionKind::Move, Some(dest), policy)
            .unwrap();

        // The local move destroyed the inbox placement — the row that held
        // the UID. Delivery must still know where the server keeps it.
        assert_eq!(store.folders_of(m.message_id).unwrap(), vec![dest]);
        let pending = store.pending_actions(account).unwrap();
        let mv = pending
            .iter()
            .find(|p| p.kind_json.contains("move"))
            .expect("queued");
        assert_eq!(mv.uid, Some(77), "address captured at queue time");
        assert_eq!(mv.folder_path, "INBOX");
    }
}

/// An action carries every message of a conversation, and each reaches the
/// server on its own. The action used to be marked sent on the first success,
/// so the rest were never retried. Now each message records its outcome and
/// the action settles on the last one: sent if any got through, undeliverable
/// only if none did.
#[test]
fn an_action_settles_only_when_every_message_has_an_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let ids = ingest_reply_chain(&mut store, &blobs, account, inbox, 3);
    let thread = store.thread_of(ids[0]).unwrap().unwrap();
    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let action = receipt.action_id;
    let queued: Vec<i64> = store
        .pending_actions(account)
        .unwrap()
        .iter()
        .map(|p| p.message_id)
        .collect();
    assert_eq!(queued.len(), 3, "{queued:?}");

    assert!(!store.mark_message_outcome(action, queued[0], true).unwrap());
    assert_eq!(
        store.action_state(action).unwrap().as_deref(),
        Some("queued")
    );
    let left: Vec<i64> = store
        .pending_actions(account)
        .unwrap()
        .iter()
        .map(|p| p.message_id)
        .collect();
    assert_eq!(
        left,
        queued[1..].to_vec(),
        "the delivered message leaves the queue"
    );

    assert!(
        !store
            .mark_message_outcome(action, queued[1], false)
            .unwrap()
    );
    assert!(store.mark_message_outcome(action, queued[2], true).unwrap());
    assert_eq!(store.action_state(action).unwrap().as_deref(), Some("sent"));
    assert!(store.pending_actions(account).unwrap().is_empty());
}

#[test]
fn an_action_none_of_whose_messages_had_a_server_copy_is_undeliverable() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let ids = ingest_reply_chain(&mut store, &blobs, account, inbox, 2);
    let thread = store.thread_of(ids[0]).unwrap().unwrap();
    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::Trash,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let action = receipt.action_id;
    for id in &ids {
        store.mark_message_outcome(action, *id, false).unwrap();
    }
    assert_eq!(
        store.action_state(action).unwrap().as_deref(),
        Some("undeliverable")
    );
}

#[test]
fn undo_puts_a_placement_back_with_its_uid() {
    // Restored without its UID, the placement could not take a flag change
    // from the server, was never pruned when the server dropped the
    // message, and disagreed with STATUS every cycle.
    let (mut store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    store.place_message_at(ids[0], inbox, 7).unwrap();

    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(
        store.placement_uid(ids[0], inbox).unwrap(),
        None,
        "archived out"
    );

    assert!(store.undo_action(receipt.action_id).unwrap());
    assert_eq!(
        store.placement_uid(ids[0], inbox).unwrap(),
        Some(Some(7)),
        "back where it was, UID and all"
    );
    assert!(
        store.set_flags_by_uid(inbox, 7, flags::SEEN).unwrap(),
        "a flag change from the server reaches it again"
    );
}

/// On a labels provider the message also sits in All Mail (or under a
/// label) with a UID of its own. The queue used to address an archive
/// there, the drain saw "already in that folder" and marked it delivered,
/// and the Inbox label stayed on the server.
#[test]
fn a_labels_archive_is_addressed_at_the_inbox_it_left() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let work = store.ensure_named_folder(account, "Work").unwrap();
    store.place_message_at(ids[0], inbox, 5).unwrap();
    store.place_message_at(ids[0], work, 3).unwrap();

    let r = store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::Archive,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();

    let rows: Vec<_> = store
        .pending_actions(account)
        .unwrap()
        .into_iter()
        .filter(|p| p.action_id == r.action_id)
        .collect();
    assert_eq!(rows.len(), 1, "one source per message");
    assert_eq!(rows[0].folder_path, "INBOX", "the folder it has to leave");
    assert_eq!(rows[0].uid, Some(5));
}

/// A flag action is not a move: it is delivered wherever the message sits,
/// and a message under two labels still gets one row per placement.
#[test]
fn a_flag_action_keeps_a_row_per_placement() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let work = store.ensure_named_folder(account, "Work").unwrap();
    store.place_message_at(ids[0], inbox, 5).unwrap();
    store.place_message_at(ids[0], work, 3).unwrap();

    let r = store
        .apply_thread_action(
            account,
            thread_of(&store, ids[0]),
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();
    let rows = store
        .pending_actions(account)
        .unwrap()
        .into_iter()
        .filter(|p| p.action_id == r.action_id)
        .count();
    assert_eq!(rows, 2);
}

/// Undo puts back the part of the snapshot the action changed, not the whole
/// snapshot. Mark read, archive, undo the mark-read: unread again, and still
/// archived. Restoring everything walked it back into the inbox, which a
/// toast outliving the next action could do.
#[test]
fn undoing_an_earlier_action_leaves_a_later_one_alone() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    store.place_message(ids[0], inbox).unwrap();
    let tid = thread_of(&store, ids[0]);

    let read = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::MarkRead,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let archived = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert!(store.undo_action(read.action_id).unwrap());
    assert_eq!(
        store.flags_of(ids[0]).unwrap() & flags::SEEN,
        0,
        "unread again"
    );
    assert!(
        !store.folders_of(ids[0]).unwrap().contains(&inbox),
        "and still archived"
    );

    // The other way round: a star set after the archive survives undoing it.
    store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Star,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert!(store.undo_action(archived.action_id).unwrap());
    assert!(
        store.folders_of(ids[0]).unwrap().contains(&inbox),
        "back in the inbox"
    );
    assert_ne!(
        store.flags_of(ids[0]).unwrap() & flags::FLAGGED,
        0,
        "still starred"
    );
}

/// Archiving a conversation takes it out of the inbox. It does not take your
/// own reply out of Sent, or pull a message out of the bin.
#[test]
fn archiving_a_conversation_leaves_sent_and_binned_messages_where_they_are() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    let ids = ingest_reply_chain(&mut store, &blobs, account, inbox, 3);
    // The middle message is your reply, filed in Sent; the last one was binned.
    store.remove_placement(ids[1], account, "INBOX").unwrap();
    store.place_message_at(ids[1], sent, 9).unwrap();
    store.remove_placement(ids[2], account, "INBOX").unwrap();
    store.place_message(ids[2], trash).unwrap();
    let tid = thread_of(&store, ids[0]);

    let r = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    assert_eq!(r.message_count, 1, "only the inbox message moved");
    assert!(!store.folders_of(ids[0]).unwrap().contains(&inbox));
    assert_eq!(
        store.folders_of(ids[1]).unwrap(),
        vec![sent],
        "the reply stays in Sent"
    );
    assert_eq!(
        store.folders_of(ids[2]).unwrap(),
        vec![trash],
        "the binned one stays binned"
    );
    let queued: Vec<_> = store
        .pending_actions(account)
        .unwrap()
        .into_iter()
        .filter(|p| p.action_id == r.action_id)
        .map(|p| p.message_id)
        .collect();
    assert_eq!(
        queued,
        vec![ids[0]],
        "and the server is asked about that one only"
    );

    // Trash is exclusive whatever the provider: it takes the reply too.
    let t = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Trash,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(t.message_count, 3);
    assert_eq!(store.folders_of(ids[1]).unwrap(), vec![trash]);
}
