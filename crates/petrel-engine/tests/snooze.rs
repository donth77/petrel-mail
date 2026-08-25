//! Snooze: a conversation put aside until a time that has not arrived.
//!
//! The property that matters is that nothing schedules anything. "Show me the
//! inbox" already means "not snoozed past now", so a conversation comes back
//! because the clock moved — there is no timer to miss, nothing to catch up on
//! after the app was shut for a week, and no way for a queue and a mailbox to
//! disagree about what should be visible.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{ListView, NewMessage, Store};

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
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    (store, account, ids)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn subjects(store: &Store, view: &ListView) -> Vec<String> {
    let mut s: Vec<String> = store
        .list_threads(view, 0, 50)
        .unwrap()
        .into_iter()
        .map(|t| t.subject)
        .collect();
    s.sort();
    s
}

fn snooze(store: &Store, account: i64, id: i64, until: i64) -> i64 {
    let tid = store.thread_of(id).unwrap().unwrap_or(-id);
    store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Snooze,
            Some(until),
            PlacementPolicy::Exclusive,
        )
        .unwrap()
        .action_id
}

#[test]
fn a_snoozed_conversation_leaves_the_inbox_for_the_snoozed_view() {
    let (store, account, ids) = seeded();
    snooze(&store, account, ids[0], now_ms() + 60_000);

    assert_eq!(subjects(&store, &ListView::Inbox), ["m1", "m2"]);
    assert_eq!(subjects(&store, &ListView::Snoozed), ["m0"]);
}

#[test]
fn it_comes_back_because_the_clock_moved_not_because_a_job_ran() {
    let (store, account, ids) = seeded();
    // Due in the past: nothing has run since, and nothing needs to.
    snooze(&store, account, ids[0], now_ms() - 1_000);

    assert_eq!(subjects(&store, &ListView::Inbox), ["m0", "m1", "m2"]);
    assert!(subjects(&store, &ListView::Snoozed).is_empty());
}

#[test]
fn undoing_a_snooze_brings_it_straight_back() {
    let (store, account, ids) = seeded();
    let action = snooze(&store, account, ids[0], now_ms() + 60_000);
    assert_eq!(subjects(&store, &ListView::Snoozed), ["m0"]);

    assert!(
        store.undo_action(action).unwrap(),
        "a local action is still undoable"
    );
    assert_eq!(subjects(&store, &ListView::Inbox).len(), 3);
    assert!(subjects(&store, &ListView::Snoozed).is_empty());
}

#[test]
fn re_snoozing_and_undoing_restores_the_previous_time_rather_than_clearing_it() {
    let (store, account, ids) = seeded();
    let first_due = now_ms() + 60_000;
    snooze(&store, account, ids[0], first_due);
    let second = snooze(&store, account, ids[0], now_ms() + 600_000);

    store.undo_action(second).unwrap();
    // Still snoozed, to the original time — not dumped back in the inbox.
    assert_eq!(subjects(&store, &ListView::Snoozed), ["m0"]);
    assert_eq!(subjects(&store, &ListView::Inbox), ["m1", "m2"]);
}

/// Snooze never reaches a server, so it must never sit in the delivery queue:
/// a stuck action there would also stop resync trusting the server about that
/// message, permanently.
#[test]
fn a_snooze_is_never_queued_for_delivery() {
    let (store, account, ids) = seeded();
    snooze(&store, account, ids[0], now_ms() + 60_000);

    assert!(store.pending_actions(account).unwrap().is_empty());
    assert!(!store.message_has_pending(ids[0]).unwrap());
}
