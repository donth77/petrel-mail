//! The outbox as rows in the store: what is due, what is held, what goes back.
//!
//! The reconciliation *rule* is proved in `outbox.rs` and against a real
//! fault-injected server in `ambiguous_send`. This is the other half — that the
//! store honours it. A message held for a person must never come back from
//! `due_sends` on its own, because the worker sends whatever that returns.

use petrel_engine::outbox::SendState;
use petrel_engine::store::Store;

fn queued(store: &Store, account: i64, at_ms: i64) -> i64 {
    let id = store
        .save_draft(
            account,
            None,
            "sam@example.com",
            "Board pack v4",
            "body",
            "",
        )
        .unwrap();
    store.schedule_send(id, Some(at_ms)).unwrap();
    id
}

#[test]
fn a_message_held_for_a_person_is_never_due() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);

    // Before anything happened it was due...
    assert_eq!(store.due_sends(account, 5_000).unwrap().len(), 1);

    // ...and once its outcome is unknown and unprovable, it is not — however
    // long it waits. Sending it could send the board pack twice.
    store
        .set_send_state(
            id,
            SendState::NeedsAttention,
            Some("socket closed"),
            None,
            Some("<m@x>"),
        )
        .unwrap();
    assert!(store.due_sends(account, 5_000).unwrap().is_empty());
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());

    // It is still in the outbox, saying so.
    let rows = store.outbox(account).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "NeedsAttention");
    assert_eq!(rows[0].error.as_deref(), Some("socket closed"));
}

#[test]
fn a_permanent_rejection_waits_for_the_person_too() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(
            id,
            SendState::FailedPermanent,
            Some("550 no such user"),
            None,
            None,
        )
        .unwrap();
    // Retrying a 550 gets another 550. It is edit or discard, not wait.
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());
}

#[test]
fn a_retry_waits_for_its_turn_and_counts_its_attempts() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);

    store
        .set_send_state(
            id,
            SendState::RetryQueued,
            Some("connect refused"),
            Some(60_000),
            None,
        )
        .unwrap();
    assert!(
        store.due_sends(account, 30_000).unwrap().is_empty(),
        "not yet"
    );
    assert_eq!(store.due_sends(account, 60_000).unwrap().len(), 1, "now");

    let row = &store.outbox(account).unwrap()[0];
    assert_eq!(row.attempts, 1);
    assert_eq!(row.next_ms, Some(60_000));
}

#[test]
fn a_person_deciding_is_what_moves_a_held_message() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(
            id,
            SendState::NeedsAttention,
            Some("socket closed"),
            None,
            Some("<m@x>"),
        )
        .unwrap();

    // "Send anyway": they looked, and decided.
    store.resend_now(id, 9_000).unwrap();
    let due = store.due_sends(account, 9_000).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(store.outbox(account).unwrap()[0].state, "RetryQueued");
    assert!(
        store.outbox(account).unwrap()[0].error.is_none(),
        "the old error is cleared"
    );
}

#[test]
fn editing_takes_it_out_of_the_outbox_and_keeps_the_text() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(id, SendState::FailedPermanent, Some("550"), None, None)
        .unwrap();

    store.unschedule_send(id).unwrap();

    assert!(store.outbox(account).unwrap().is_empty());
    let draft = store.load_draft(id).unwrap();
    assert_eq!(draft.subject, "Board pack v4");
    assert_eq!(draft.body, "body");
}

#[test]
fn the_message_id_is_kept_across_states() {
    // It is what a later "check again" searches Sent for, so a state change
    // that dropped it would make the message un-checkable.
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(id, SendState::Transmitting, None, None, Some("<m@x>"))
        .unwrap();
    store
        .set_send_state(id, SendState::NeedsAttention, Some("dropped"), None, None)
        .unwrap();
    assert_eq!(
        store.conn_query_send_message_id(id).unwrap().as_deref(),
        Some("<m@x>")
    );
}

#[test]
fn an_interrupted_transmit_is_held_for_a_person() {
    // Transmitting is not due, and the UI has no button for it. A process
    // that died mid-SMTP would otherwise leave the row there forever. The
    // body may already have gone, so this is NeedsAttention rather than a
    // retry the engine cannot prove safe.
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let id = queued(&store, account, 1_000);
    store
        .set_send_state(id, SendState::Transmitting, None, None, Some("<m@x>"))
        .unwrap();
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());

    assert_eq!(store.recover_interrupted_sends().unwrap(), 1);
    assert!(store.due_sends(account, i64::MAX).unwrap().is_empty());
    let rows = store.outbox(account).unwrap();
    assert_eq!(rows[0].state, "NeedsAttention");
    assert_eq!(
        store.conn_query_send_message_id(id).unwrap().as_deref(),
        Some("<m@x>")
    );
}

#[test]
fn the_clock_knows_when_to_wake() {
    // The drain is not clock-driven on its own: it runs when a triage action
    // asks or when the sync comes round, and with IDLE the sync sleeps until
    // the server pushes. A scheduled message therefore needs its own alarm,
    // and this is what the alarm reads.
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    assert_eq!(
        store.next_due_ms(account).unwrap(),
        None,
        "empty outbox: nothing to wake for"
    );

    let a = queued(&store, account, 5_000);
    let b = queued(&store, account, 2_000);
    assert_eq!(
        store.next_due_ms(account).unwrap(),
        Some(2_000),
        "the earliest"
    );

    // A retry pushes its own message later; the other is still first.
    store
        .set_send_state(
            b,
            SendState::RetryQueued,
            Some("refused"),
            Some(9_000),
            None,
        )
        .unwrap();
    assert_eq!(store.next_due_ms(account).unwrap(), Some(5_000));

    // A message held for a person has no time, only a person.
    store
        .set_send_state(
            a,
            SendState::NeedsAttention,
            Some("unknown"),
            None,
            Some("<m@x>"),
        )
        .unwrap();
    assert_eq!(
        store.next_due_ms(account).unwrap(),
        Some(9_000),
        "only the retry remains"
    );
}
