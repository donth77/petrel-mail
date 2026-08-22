//! The rail's views, as queries.
//!
//! The property under test is that a conversation appears in exactly the views
//! it belongs to — and, just as importantly, that mail the sync has not filed
//! anywhere still shows up in the inbox. A "positive" inbox filter (placed in
//! the inbox folder) passes every other test here and empties the mailbox in
//! production, which is the failure this file exists to prevent.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{ListView, NewMessage, Store, flags};

fn seeded() -> (Store, i64, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = (0..4)
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

#[test]
fn parse_maps_rail_keys_and_never_errors() {
    assert_eq!(ListView::parse("inbox"), ListView::Inbox);
    assert_eq!(ListView::parse("starred"), ListView::Starred);
    assert_eq!(
        ListView::parse("archive"),
        ListView::Folder("archive".into())
    );
    assert_eq!(ListView::parse("trash"), ListView::Folder("trash".into()));
    assert_eq!(
        ListView::parse("tag:urgent"),
        ListView::Tag("urgent".into())
    );
    assert_eq!(ListView::parse("snoozed"), ListView::Snoozed);
    assert_eq!(ListView::parse("outbox"), ListView::Outbox);
    // A stale or unknown view falls back rather than failing: the worst
    // outcome of a bad rail key should be the wrong list, not a broken screen.
    assert_eq!(ListView::parse("nonsense"), ListView::Inbox);
    assert_eq!(ListView::parse("tag:"), ListView::Inbox);
}

#[test]
fn archiving_moves_a_conversation_between_the_inbox_and_archive_views() {
    let (store, account, ids) = seeded();
    let tid = thread_of(&store, ids[0]);

    assert_eq!(subjects(&store, &ListView::Inbox).len(), 4);
    assert!(subjects(&store, &ListView::Folder("archive".into())).is_empty());

    let receipt = store
        .apply_thread_action(
            account,
            tid,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    assert_eq!(subjects(&store, &ListView::Inbox), ["m1", "m2", "m3"]);
    assert_eq!(
        subjects(&store, &ListView::Folder("archive".into())),
        ["m0"]
    );

    store.undo_action(receipt.action_id).unwrap();
    assert_eq!(subjects(&store, &ListView::Inbox).len(), 4);
    assert!(subjects(&store, &ListView::Folder("archive".into())).is_empty());
}

#[test]
fn trash_and_spam_are_separate_views() {
    let (store, account, ids) = seeded();
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

    assert_eq!(subjects(&store, &ListView::Folder("trash".into())), ["m0"]);
    assert_eq!(subjects(&store, &ListView::Folder("spam".into())), ["m1"]);
    assert_eq!(subjects(&store, &ListView::Inbox), ["m2", "m3"]);
}

#[test]
fn starred_spans_folders_but_not_the_bin() {
    let (store, account, ids) = seeded();
    for id in &ids[..3] {
        store.set_flags(*id, flags::FLAGGED, 0).unwrap();
    }
    // Starred mail keeps its star when archived: Starred is a view across the
    // mailbox, not a folder you move things into.
    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[1]),
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    // ...but trashing something takes it out of Starred. A star is not a reason
    // to keep showing you mail you threw away.
    store
        .apply_thread_action(
            account,
            thread_of(&store, ids[2]),
            ActionKind::Trash,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    assert_eq!(subjects(&store, &ListView::Starred), ["m0", "m1"]);
}

#[test]
fn tag_views_select_by_tag_and_are_not_sql() {
    let (store, account, ids) = seeded();
    let tag = store.ensure_tag(account, "urgent", None).unwrap();
    store.tag_message(ids[2], tag).unwrap();

    assert_eq!(subjects(&store, &ListView::Tag("urgent".into())), ["m2"]);
    assert!(subjects(&store, &ListView::Tag("nope".into())).is_empty());

    // The tag name reaches the query as a bound parameter. If it were
    // interpolated this would either error or empty the table.
    let hostile = ListView::Tag("' OR 1=1 --".into());
    assert!(subjects(&store, &hostile).is_empty());
    assert_eq!(subjects(&store, &ListView::Inbox).len(), 4, "table intact");
}

/// Gmail has no Archive folder: archiving removes the Inbox label and the
/// message stays in All Mail, which is mapped to the archive role. So the
/// Archive *view* has to mean "not in the inbox" — otherwise, once All Mail is
/// synced, every message in it (which is all of them) would appear archived.
#[test]
fn archive_excludes_anything_still_in_the_inbox() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let all_mail = store
        .ensure_folder(account, "archive", "[Gmail]/All Mail")
        .unwrap();

    // What syncing All Mail looks like: everything is in it, and the inbox
    // messages are in both.
    for id in &ids {
        store.place_message(*id, all_mail).unwrap();
    }
    store.place_message(ids[0], inbox).unwrap();
    store.place_message(ids[1], inbox).unwrap();

    let archived = subjects(&store, &ListView::Folder("archive".into()));
    assert_eq!(
        archived,
        ["m2", "m3"],
        "inbox mail must not read as archived"
    );
}
