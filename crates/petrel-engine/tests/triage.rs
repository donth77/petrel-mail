//! Triage actions apply locally at once and are undoable exactly.
//!
//! The property under test throughout is that undo restores *what was there*,
//! not an inferred inverse — the difference only shows up once more than one
//! action has touched the same message, which is where guessing goes wrong.

use petrel_engine::actions::ActionKind;
use petrel_engine::store::{NewMessage, Store, flags};

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
        .apply_thread_action(account, tid, ActionKind::Archive)
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
        .apply_thread_action(account, tid, ActionKind::MarkUnread)
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
        .apply_thread_action(account, tid, ActionKind::Star)
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
        .apply_thread_action(account, tid, ActionKind::Star)
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
        .apply_thread_action(account, tid, ActionKind::Star)
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

    let before = store.list_threads(0, 50).unwrap();
    assert_eq!(before.len(), 3, "all three start in the listing");

    let receipt = store
        .apply_thread_action(account, tid, ActionKind::Archive)
        .unwrap();
    let after = store.list_threads(0, 50).unwrap();
    assert_eq!(after.len(), 2, "the archived conversation left the listing");
    assert!(
        !after.iter().any(|t| t.thread_id == tid),
        "archived conversation must not be listed"
    );

    store.undo_action(receipt.action_id).unwrap();
    let restored = store.list_threads(0, 50).unwrap();
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
        store.list_threads(0, 50).unwrap().len(),
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
        .apply_thread_action(account, thread_of(&store, ids[0]), ActionKind::Trash)
        .unwrap();
    store
        .apply_thread_action(account, thread_of(&store, ids[1]), ActionKind::Spam)
        .unwrap();
    assert_eq!(store.list_threads(0, 50).unwrap().len(), 1);
}
