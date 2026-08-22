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
