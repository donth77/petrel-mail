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

    let before = store.list_threads(&ListView::Inbox, 0, 50).unwrap();
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
    let after = store.list_threads(&ListView::Inbox, 0, 50).unwrap();
    assert_eq!(after.len(), 2, "the archived conversation left the listing");
    assert!(
        !after.iter().any(|t| t.thread_id == tid),
        "archived conversation must not be listed"
    );

    store.undo_action(receipt.action_id).unwrap();
    let restored = store.list_threads(&ListView::Inbox, 0, 50).unwrap();
    assert_eq!(restored.len(), 3, "undo puts it back in the listing");
    assert!(restored.iter().any(|t| t.thread_id == tid));
}

/// Mail that was never filed anywhere is still mail. An inbox filter written as
/// "placed in the inbox folder" would hide every message the sync had not yet
/// placed, which is the same class of bug as joining on a NULL thread_id.
#[test]
fn unplaced_messages_stay_in_the_listing() {
    let (store, _account, _ids) = seeded();
    assert_eq!(
        store.list_threads(&ListView::Inbox, 0, 50).unwrap().len(),
        3,
        "messages with no placement at all must still be listed"
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
        store.list_threads(&ListView::Inbox, 0, 50).unwrap().len(),
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
            .list_threads(&ListView::Folder("archive".into()), 0, 50)
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
