//! Sparse views must page from their membership, not the mailbox.
//!
//! Walking every message looking for a sent or starred row is the plan
//! the counts used to take — 300ms to find thirteen drafts. These tests
//! keep a large inbox next to a handful of matching rows so a walk of
//! the mailbox would still *work*, and only the membership path stays
//! cheap. Correctness is what we can assert here: the list is those
//! rows, and only those rows.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{CountMode, ListView, NewMessage, Store, flags};

fn store() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let account = s.ensure_test_account().unwrap();
    (s, account)
}

fn fill_inbox(store: &mut Store, account: i64, n: i64) -> Vec<i64> {
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let msgs: Vec<NewMessage> = (0..n)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 10_000 + i,
            from_addr: "a@example.com".into(),
            from_display: "A".into(),
            to_addr: "me@example.com".into(),
            subject: format!("inbox-{i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    ids
}

fn subjects(store: &Store, view: &ListView) -> Vec<String> {
    store
        .list_threads(view, 0, 50, petrel_engine::store::Sort::default())
        .unwrap()
        .into_iter()
        .map(|r| r.subject)
        .collect()
}

fn place_named(store: &mut Store, account: i64, folder: i64, subject: &str, date_ms: i64) -> i64 {
    let ids = store
        .insert_messages(&[NewMessage {
            account_id: account,
            date_ms,
            from_addr: "b@example.com".into(),
            from_display: "B".into(),
            to_addr: "me@example.com".into(),
            subject: subject.into(),
            body_text: "body".into(),
        }])
        .unwrap();
    store.place_message(ids[0], folder).unwrap();
    ids[0]
}

#[test]
fn sent_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 200);
    let sent = s.ensure_folder(account, "sent", "Sent").unwrap();
    place_named(&mut s, account, sent, "old-sent", 1_000);
    place_named(&mut s, account, sent, "new-sent", 2_000);

    let found = subjects(&s, &ListView::Folder("sent".into()));
    assert_eq!(found.len(), 2);
    assert!(found.contains(&"old-sent".into()));
    assert!(found.contains(&"new-sent".into()));
    assert!(
        !found.iter().any(|s| s.starts_with("inbox-")),
        "inbox mail must not leak into sent"
    );
}

#[test]
fn spam_and_trash_lists_find_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 200);
    let spam = s.ensure_folder(account, "spam", "Spam").unwrap();
    let trash = s.ensure_folder(account, "trash", "Trash").unwrap();
    place_named(&mut s, account, spam, "junk", 1_000);
    place_named(&mut s, account, trash, "gone", 2_000);

    assert_eq!(subjects(&s, &ListView::Folder("spam".into())), ["junk"]);
    assert_eq!(subjects(&s, &ListView::Folder("trash".into())), ["gone"]);
}

#[test]
fn a_user_folder_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 200);
    let filed = s.ensure_folder(account, "", "Contracts").unwrap();
    place_named(&mut s, account, filed, "contract", 1_000);

    let found = subjects(&s, &ListView::UserFolder(filed));
    assert_eq!(found, ["contract"]);
}

#[test]
fn a_tag_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    let inbox_ids = fill_inbox(&mut s, account, 200);
    let tag = s.ensure_tag(account, "Urgent", None).unwrap();
    s.tag_message(inbox_ids[3], tag).unwrap();
    s.tag_message(inbox_ids[9], tag).unwrap();

    let mut found = subjects(&s, &ListView::Tag("Urgent".into()));
    found.sort();
    assert_eq!(found, ["inbox-3", "inbox-9"]);
}

#[test]
fn starred_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    let inbox_ids = fill_inbox(&mut s, account, 200);
    s.set_flags(inbox_ids[5], flags::FLAGGED, 0).unwrap();
    s.set_flags(inbox_ids[17], flags::FLAGGED, 0).unwrap();

    let mut found = subjects(&s, &ListView::Starred);
    found.sort();
    assert_eq!(found, ["inbox-17", "inbox-5"]);
}

#[test]
fn snoozed_list_finds_a_few_among_many_inbox_messages() {
    let (mut s, account) = store();
    let inbox_ids = fill_inbox(&mut s, account, 200);
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        + 600_000;
    let tid = s.thread_of(inbox_ids[8]).unwrap().unwrap_or(-inbox_ids[8]);
    s.apply_thread_action(
        account,
        tid,
        ActionKind::Snooze,
        Some(until),
        PlacementPolicy::Exclusive,
    )
    .unwrap();

    assert_eq!(subjects(&s, &ListView::Snoozed), ["inbox-8"]);
}

/// A tag's name is unique per account, not per store. The tag page used to
/// collect the other account's conversations too, and since the row query
/// keeps the account, they took up the page and then vanished from it: an
/// account with three Urgent conversations opened the tag and saw none.
#[test]
fn a_tag_list_stays_inside_the_active_account() {
    let (mut s, mine) = store();
    let theirs = s.ensure_test_account().unwrap();
    s.set_active_account(mine).unwrap();
    let my_ids = fill_inbox(&mut s, mine, 10);
    let their_inbox = s.ensure_folder(theirs, "inbox", "INBOX").unwrap();
    // Newer than mine, and more than a page of them, so a walk that forgot
    // the account would fill the page with theirs and mine would fall off.
    let their_msgs: Vec<NewMessage> = (0..60)
        .map(|i| NewMessage {
            account_id: theirs,
            date_ms: 50_000 + i,
            from_addr: "c@example.com".into(),
            from_display: "C".into(),
            to_addr: "them@example.com".into(),
            subject: format!("theirs-{i}"),
            body_text: "body".into(),
        })
        .collect();
    let their_ids = s.insert_messages(&their_msgs).unwrap();
    for id in &their_ids {
        s.place_message(*id, their_inbox).unwrap();
    }
    let my_tag = s.ensure_tag(mine, "Urgent", None).unwrap();
    let their_tag = s.ensure_tag(theirs, "Urgent", None).unwrap();
    for id in &my_ids[..3] {
        s.tag_message(*id, my_tag).unwrap();
    }
    for id in &their_ids {
        s.tag_message(*id, their_tag).unwrap();
    }

    let view = ListView::Tag("Urgent".into());
    let mut found = subjects(&s, &view);
    found.sort();
    assert_eq!(found, ["inbox-0", "inbox-1", "inbox-2"]);
    assert_eq!(
        s.count_view(&view, true).unwrap(),
        3,
        "the count and the page must agree"
    );
}

/// A saved search's badge counts every match, not a page of the ranking.
///
/// This is the whole reason `count_search` is not `search_threads(..).len()`:
/// the search walks at most six hundred hits, so a query matching a mailbox
/// would answer six hundred for ever, and a badge reading "600" beside a
/// mailbox of two thousand is a number that is simply wrong. Eight hundred
/// messages is past that page and cheap to build.
#[test]
fn a_count_is_not_capped_by_the_ranking_page() {
    let (mut s, account) = store();
    fill_inbox(&mut s, account, 800);

    // Words, so the query goes down the ranked path where the cap lives.
    assert_eq!(s.search_threads("body", 50).unwrap().len(), 50);
    assert_eq!(s.count_search("body", CountMode::Total).unwrap(), 800);

    // And down the conditions-only path, which pages the same way.
    assert_eq!(s.count_search("is:unread", CountMode::Total).unwrap(), 800);

    // Reading some moves the unread count and leaves the total alone.
    for id in s.search_threads("body", 20).unwrap().iter().take(20) {
        s.set_flags(id.id, flags::SEEN, 0).unwrap();
    }
    assert_eq!(s.count_search("body", CountMode::Total).unwrap(), 800);
    assert_eq!(s.count_search("body", CountMode::Unread).unwrap(), 780);
}
