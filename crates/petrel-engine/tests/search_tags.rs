//! A search result is still a row, and a row carries its tags.
//!
//! Search builds its rows through a different query than the list does, and the
//! two had drifted: one selected the tag's id alongside its name and colour, the
//! other selected only name and colour. The row type needs all three, so the
//! shorter one did not fail loudly — it failed to parse, and the parse helper
//! turns a failure into an empty list. Every tag simply vanished from every
//! search result, with nothing logged and nothing to see but rows that looked
//! untagged.

use petrel_engine::store::{NewMessage, Store};

fn seeded() -> (Store, i64, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs = [
        ("Q3 vendor contracts", "The annex and the pricing sheet."),
        ("Draft terms", "I marked up the annex in track changes."),
    ]
    .iter()
    .map(|(subject, body)| NewMessage {
        account_id: account,
        date_ms: 1_700_000_000_000,
        from_addr: "sam@example.com".into(),
        from_display: "Sam Ortiz".into(),
        to_addr: "me@example.com".into(),
        subject: (*subject).into(),
        body_text: (*body).into(),
    })
    .collect::<Vec<_>>();
    let ids = store.insert_messages(&msgs).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    (store, account, ids)
}

#[test]
fn a_search_result_carries_the_tags_the_row_has() {
    let (store, account, ids) = seeded();
    let tag = store
        .ensure_tag(account, "Urgent", Some("#A8544B"))
        .unwrap();
    store.tag_message(ids[0], tag).unwrap();

    let hits = store.search_threads("annex", 20).unwrap();
    let tagged = hits
        .iter()
        .find(|h| h.subject == "Q3 vendor contracts")
        .expect("the tagged message is among the hits");

    assert_eq!(
        tagged.tags.len(),
        1,
        "the tag is on the row, so it has to survive the search query too"
    );
    assert_eq!(tagged.tags[0].name, "Urgent");
    assert_eq!(tagged.tags[0].colour, "#A8544B");
}

/// The id is the part that went missing, and the part a row cannot do without:
/// untagging from a row names the tag by id rather than looking it up by name.
#[test]
fn a_search_result_can_name_its_tag_to_the_engine() {
    let (store, account, ids) = seeded();
    let tag = store
        .ensure_tag(account, "Urgent", Some("#A8544B"))
        .unwrap();
    store.tag_message(ids[0], tag).unwrap();

    let hits = store.search_threads("annex", 20).unwrap();
    let tagged = hits
        .iter()
        .find(|h| h.subject == "Q3 vendor contracts")
        .expect("the tagged message is among the hits");

    assert_eq!(
        tagged.tags.first().map(|t| t.id),
        Some(tag),
        "a chip the reader can see has to be one they can remove"
    );
}

/// The list path and the search path describe the same row, so they must not
/// disagree about what is on it.
#[test]
fn the_list_and_the_search_agree_about_a_row() {
    let (store, account, ids) = seeded();
    let tag = store
        .ensure_tag(account, "Urgent", Some("#A8544B"))
        .unwrap();
    store.tag_message(ids[0], tag).unwrap();

    let listed = store
        .list_threads(
            &petrel_engine::store::ListView::Inbox,
            0,
            20,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    let from_list = listed
        .iter()
        .find(|r| r.subject == "Q3 vendor contracts")
        .expect("in the inbox");
    let hits = store.search_threads("annex", 20).unwrap();
    let from_search = hits
        .iter()
        .find(|h| h.subject == "Q3 vendor contracts")
        .expect("in the results");

    let names = |ts: &[petrel_engine::store::ThreadRowTag]| {
        ts.iter()
            .map(|t| (t.id, t.name.clone(), t.colour.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&from_list.tags), names(&from_search.tags));
}
