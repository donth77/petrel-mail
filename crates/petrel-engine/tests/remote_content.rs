//! Who is allowed to load remote content, and why.
//!
//! The rule this file exists to pin down: blocked by default, trusted by hand,
//! or trusted because the user wrote to them first. That third one is what
//! makes blocking liveable rather than a setting everybody turns off, so it has
//! to be right — and it must not be satisfied by merely *receiving* mail, which
//! would let any stranger buy their way in by sending twice.

use petrel_engine::store::{NewMessage, Store};

fn seeded() -> (Store, i64, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = ["stranger@example.com", "colleague@example.com"]
        .iter()
        .enumerate()
        .map(|(i, from)| NewMessage {
            account_id: account,
            date_ms: 1_000 + i as i64,
            from_addr: (*from).into(),
            from_display: "Someone".into(),
            to_addr: "me@example.com".into(),
            subject: format!("m{i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    (store, account, ids)
}

#[test]
fn blocked_by_default() {
    let (store, _account, ids) = seeded();
    assert!(!store.remote_content_allowed(ids[0]).unwrap());
    assert!(!store.remote_content_allowed(ids[1]).unwrap());
}

#[test]
fn trusting_a_sender_is_remembered_and_reversible() {
    let (store, account, ids) = seeded();
    store
        .trust_sender(account, "stranger@example.com", 5)
        .unwrap();
    assert!(store.remote_content_allowed(ids[0]).unwrap());
    // Only that one. Trust is per sender, not a switch.
    assert!(!store.remote_content_allowed(ids[1]).unwrap());
    assert_eq!(
        store.trusted_senders(account).unwrap(),
        ["stranger@example.com"]
    );

    store
        .untrust_sender(account, "stranger@example.com")
        .unwrap();
    assert!(!store.remote_content_allowed(ids[0]).unwrap());
    assert!(store.trusted_senders(account).unwrap().is_empty());
}

#[test]
fn case_and_whitespace_do_not_defeat_trust() {
    let (store, account, ids) = seeded();
    store
        .trust_sender(account, "  STRANGER@Example.com  ", 5)
        .unwrap();
    assert!(
        store.remote_content_allowed(ids[0]).unwrap(),
        "a trusted address must match however it was typed"
    );
}

#[test]
fn writing_to_someone_lets_their_images_through() {
    let (mut store, account, ids) = seeded();
    let sent = store
        .ensure_folder(account, "sent", "[Gmail]/Sent Mail")
        .unwrap();

    // A message the user sent to the colleague, filed in Sent.
    let reply = store
        .insert_messages(&[NewMessage {
            account_id: account,
            date_ms: 2_000,
            from_addr: "me@example.com".into(),
            from_display: "Me".into(),
            to_addr: "colleague@example.com".into(),
            subject: "re: m1".into(),
            body_text: "on my way".into(),
        }])
        .unwrap()[0];
    store.place_message(reply, sent).unwrap();

    assert!(
        store.remote_content_allowed(ids[1]).unwrap(),
        "someone the user has written to already knows they exist"
    );
    assert!(
        !store.remote_content_allowed(ids[0]).unwrap(),
        "and nobody else is let in by it"
    );
}

#[test]
fn merely_being_written_to_earns_nothing() {
    let (mut store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();

    // The stranger writes again, and again. Received mail is not consent: if it
    // were, any sender could trust themselves by sending twice.
    let more = store
        .insert_messages(&[NewMessage {
            account_id: account,
            date_ms: 3_000,
            from_addr: "stranger@example.com".into(),
            from_display: "Someone".into(),
            to_addr: "me@example.com".into(),
            subject: "hello again".into(),
            body_text: "still here".into(),
        }])
        .unwrap()[0];
    store.place_message(more, inbox).unwrap();

    assert!(!store.remote_content_allowed(ids[0]).unwrap());
    assert!(!store.remote_content_allowed(more).unwrap());
}

#[test]
fn a_draft_to_someone_is_not_having_written_to_them() {
    let (mut store, account, ids) = seeded();
    let drafts = store
        .ensure_folder(account, "drafts", "[Gmail]/Drafts")
        .unwrap();

    let unsent = store
        .insert_messages(&[NewMessage {
            account_id: account,
            date_ms: 2_000,
            from_addr: "me@example.com".into(),
            from_display: "Me".into(),
            to_addr: "colleague@example.com".into(),
            subject: "half written".into(),
            body_text: "…".into(),
        }])
        .unwrap()[0];
    store.place_message(unsent, drafts).unwrap();

    assert!(
        !store.remote_content_allowed(ids[1]).unwrap(),
        "a draft has not reached anyone, so it tells the sender nothing"
    );
}
