//! Tags as things you can keep: rename them, colour them, throw them away.
//!
//! A tag is a name a person chose, so it will be misspelled, re-thought and
//! abandoned. Being unable to correct one is what turns a tag list into a pile
//! of near-duplicates nobody trusts.

use petrel_engine::store::{ListView, NewMessage, Store};

fn seeded() -> (Store, i64, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = (0..2)
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

#[test]
fn renaming_keeps_every_message_that_carries_it() {
    let (store, account, ids) = seeded();
    let tag = store.ensure_tag(account, "Urgnet", None).unwrap();
    store.tag_message(ids[0], tag).unwrap();

    store.rename_tag(tag, "Urgent").unwrap();

    let tags = store.tags_for_account(account).unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "Urgent");
    // The message is tagged with the id, so a rename cannot detach it.
    assert_eq!(tags[0].thread_count, 1);
    assert_eq!(store.tags_of(ids[0]).unwrap(), vec![tag]);
}

#[test]
fn renaming_onto_an_existing_name_is_refused() {
    // Two tags with one name are indistinguishable in the rail and in search,
    // and merging them silently is not a decision to make on someone's behalf.
    let (store, account, _ids) = seeded();
    let a = store.ensure_tag(account, "Urgent", None).unwrap();
    let _b = store.ensure_tag(account, "Waiting", None).unwrap();

    assert!(store.rename_tag(a, "Waiting").is_err());
    assert!(
        store.rename_tag(a, "waiting").is_err(),
        "case is not a difference"
    );
    // ...and nothing changed.
    let names: Vec<String> = store
        .tags_for_account(account)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, vec!["Urgent".to_string(), "Waiting".to_string()]);
}

#[test]
fn a_tag_still_answers_to_its_own_name() {
    let (store, account, _ids) = seeded();
    let tag = store.ensure_tag(account, "Urgent", None).unwrap();
    // Renaming to what it already is must not trip the clash check.
    store.rename_tag(tag, "Urgent").unwrap();
    assert!(store.rename_tag(tag, "   ").is_err(), "a tag needs a name");
}

#[test]
fn colour_is_kept_and_is_ours_alone() {
    let (store, account, _ids) = seeded();
    let tag = store.ensure_tag(account, "Urgent", None).unwrap();
    store.set_tag_colour(tag, "#c0392b").unwrap();
    assert_eq!(
        store.tags_for_account(account).unwrap()[0].colour,
        "#c0392b"
    );
}

#[test]
fn deleting_takes_it_off_the_messages_too() {
    let (store, account, ids) = seeded();
    let tag = store.ensure_tag(account, "Urgent", None).unwrap();
    store.tag_message(ids[0], tag).unwrap();
    store.tag_message(ids[1], tag).unwrap();

    store.delete_tag(tag).unwrap();

    assert!(store.tags_for_account(account).unwrap().is_empty());
    // Not left pointing at a tag that no longer exists, which would draw a
    // blank chip on every row that still referenced it.
    assert!(store.tags_of(ids[0]).unwrap().is_empty());
    assert!(store.tags_of(ids[1]).unwrap().is_empty());
    // And the view built on it is empty rather than broken.
    let rows = store
        .list_threads(&ListView::parse("tag:Urgent"), 0, 50)
        .unwrap();
    assert!(rows.is_empty());
}
