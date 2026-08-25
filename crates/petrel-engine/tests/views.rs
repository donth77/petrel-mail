//! The rail's views, as queries.
//!
//! The property under test is that a conversation appears in exactly the views
//! it belongs to — and, just as importantly, that mail the sync has not filed
//! anywhere still shows up in the inbox. A "positive" inbox filter (placed in
//! the inbox folder) passes every other test here and empties the mailbox in
//! production, which is the failure this file exists to prevent.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{CountMode, ListView, NewMessage, Store, flags};

/// Places every seeded message in the inbox — the tests that read Inbox
/// call this; the tests that build their own placements do not.
fn inbox_all(store: &Store, account: i64, ids: &[i64]) {
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in ids {
        store.place_message(*id, inbox).unwrap();
    }
}

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
    inbox_all(&store, account, &ids);
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
    inbox_all(&store, account, &ids);
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
    inbox_all(&store, account, &ids);
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

/// The rail's numbers. Unread where unread means something, a waiting count
/// where it does not, and nothing at all for Sent.
#[test]
fn view_counts_report_per_mailbox_and_by_conversation() {
    let (store, account, ids) = seeded();
    inbox_all(&store, account, &ids);
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let spam = store
        .ensure_folder(account, "spam", "[Gmail]/Spam")
        .unwrap();
    let sent = store
        .ensure_folder(account, "sent", "[Gmail]/Sent Mail")
        .unwrap();

    for id in &ids {
        store.place_message(*id, inbox).unwrap();
        store.set_flags(*id, 0, flags::SEEN).unwrap(); // unread
    }
    let counts = |s: &Store| -> std::collections::HashMap<String, i64> {
        s.view_counts(CountMode::Unread)
            .unwrap()
            .into_iter()
            .collect()
    };
    assert_eq!(counts(&store).get("inbox"), Some(&4));

    // Reading one drops the count by one, and only that one.
    store.set_flags(ids[0], flags::SEEN, 0).unwrap();
    assert_eq!(counts(&store).get("inbox"), Some(&3));

    // A view with nothing unread is absent rather than present-and-zero, so
    // the rail has nothing to render.
    store.place_message(ids[1], spam).unwrap();
    store.set_flags(ids[1], flags::SEEN, 0).unwrap();
    assert_eq!(counts(&store).get("spam"), None);

    // Sent never reports an unread count: mail you wrote is not unread in any
    // useful sense, and a number that never changes is furniture.
    store.place_message(ids[2], sent).unwrap();
    assert_eq!(counts(&store).get("sent"), None);

    // Asking for totals is a different question, and Sent can answer that one.
    let totals: std::collections::HashMap<String, i64> = store
        .view_counts(CountMode::Total)
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(totals.get("sent"), Some(&1));
    // Three, not four — and not two, which the old "not filed elsewhere"
    // inbox produced. Membership reads each placement for what it is: the
    // spam-placed message is binned and leaves, but the sent-placed one
    // still holds its inbox placement and stays — mail you sent to yourself
    // is genuinely both, and only the *filing gestures* (archive, move,
    // trash) take the inbox placement away.
    assert_eq!(totals.get("inbox"), Some(&3));

    // Off means off, not zeroes.
    assert!(store.view_counts(CountMode::Off).unwrap().is_empty());
}

/// The account header's unread is a claim about your mail, not about every row
/// in the database. Spam and the bin are mail already dealt with; counting them
/// would have the header announce work that does not exist.
#[test]
fn account_unread_ignores_spam_and_the_bin() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let spam = store
        .ensure_folder(account, "spam", "[Gmail]/Spam")
        .unwrap();
    let trash = store
        .ensure_folder(account, "trash", "[Gmail]/Trash")
        .unwrap();
    for id in &ids {
        store.set_flags(*id, 0, flags::SEEN).unwrap();
    }
    store.place_message(ids[0], inbox).unwrap();
    store.place_message(ids[1], inbox).unwrap();
    store.place_message(ids[2], spam).unwrap();
    store.place_message(ids[3], trash).unwrap();

    let summary = store.accounts().unwrap();
    let me = summary.iter().find(|a| a.id == account).unwrap();
    assert_eq!(me.unread_count, 2, "only the inbox pair counts");
}

/// A conversation in the bin is not still "Urgent".
///
/// Starred already excluded the bins; tags did not, so trashing something
/// tagged left it listed under its tag — and the list would have brought it
/// back the moment anything refreshed, whatever the UI did optimistically.
#[test]
fn tag_views_exclude_the_bins_like_starred_does() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    let spam = store.ensure_folder(account, "spam", "Spam").unwrap();
    let tag = store.ensure_tag(account, "urgent", None).unwrap();
    for id in &ids[..3] {
        store.place_message(*id, inbox).unwrap();
        store.tag_message(*id, tag).unwrap();
    }
    assert_eq!(subjects(&store, &ListView::Tag("urgent".into())).len(), 3);

    store.place_message(ids[0], trash).unwrap();
    store.place_message(ids[1], spam).unwrap();
    let left = subjects(&store, &ListView::Tag("urgent".into()));
    assert_eq!(
        left,
        ["m2"],
        "binned mail should not still be tagged urgent"
    );
}

/// A star is not a place.
///
/// Starred is synced as a folder so that a star on old or archived mail is
/// visible at all — but arriving through it says nothing about where the
/// message lives. The inbox treats a message with no known placement as being
/// in the inbox, so one that came only from Starred must be filed, or every
/// archived favourite reappears in the inbox.
#[test]
fn a_starred_placement_alone_does_not_put_a_message_in_the_inbox() {
    let (store, account, ids) = seeded();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let starred = store
        .ensure_folder(account, "starred", "[Gmail]/Starred")
        .unwrap();
    let archive = store
        .ensure_folder(account, "archive", "[Gmail]/All Mail")
        .unwrap();

    // One starred and in the inbox; one starred and archived.
    store.place_message(ids[0], inbox).unwrap();
    store.place_message(ids[0], starred).unwrap();
    store.set_flags(ids[0], flags::FLAGGED, 0).unwrap();

    store.place_message(ids[1], starred).unwrap();
    store.place_message(ids[1], archive).unwrap();
    store.set_flags(ids[1], flags::FLAGGED, 0).unwrap();

    let in_inbox = subjects(&store, &ListView::Inbox);
    assert!(
        in_inbox.contains(&"m0".to_string()),
        "the inbox one stays: {in_inbox:?}"
    );
    assert!(
        !in_inbox.contains(&"m1".to_string()),
        "the archived one must not: {in_inbox:?}"
    );

    // Both are starred, wherever they live — that is the whole point.
    let starred_view = subjects(&store, &ListView::Starred);
    assert!(starred_view.contains(&"m0".to_string()));
    assert!(starred_view.contains(&"m1".to_string()));
}

/// Gmail's labels decide where a message lives, because IMAP cannot say.
mod gmail_labels {
    use super::*;
    use petrel_engine::blob::BlobStore;

    /// Ingested through the real path, because the sweep matches on the
    /// Message-ID header and only ingest records one.
    fn held() -> (Store, BlobStore, tempfile::TempDir, i64) {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        for i in 0..3 {
            let raw = format!(
                "From: sam@example.com\r\nSubject: m{i}\r\nMessage-ID: <m{i}@example.com>\r\n\r\nbody",
            );
            store
                .ingest_raw(&blobs, account, None, None, raw.as_bytes())
                .unwrap();
        }
        (store, blobs, dir, account)
    }

    fn label(id: &str, labels: &[&str]) -> (String, Vec<String>) {
        (id.into(), labels.iter().map(|s| (*s).to_string()).collect())
    }

    #[test]
    fn a_message_without_the_inbox_label_is_archived() {
        let (store, _b, _d, account) = held();
        let n = store
            .apply_gmail_labels(
                account,
                &[
                    label("m0@example.com", &["\\Inbox"]),
                    label("m1@example.com", &["\\Important"]),
                ],
            )
            .unwrap();
        assert_eq!(n, 2);

        assert!(subjects(&store, &ListView::Inbox).contains(&"m0".to_string()));
        assert_eq!(
            subjects(&store, &ListView::Folder("archive".into())),
            ["m1"]
        );
    }

    /// The star that started all this: carried by the sweep, so it shows on
    /// mail that was never fetched from the Starred mailbox.
    #[test]
    fn the_starred_label_sets_the_flag() {
        let (store, _b, _d, account) = held();
        store
            .apply_gmail_labels(
                account,
                &[label("m2@example.com", &["\\Inbox", "\\Starred"])],
            )
            .unwrap();
        assert_eq!(subjects(&store, &ListView::Starred), ["m2"]);
    }

    /// Unstarring on the server has to reach us too, or a star can be set and
    /// never cleared.
    #[test]
    fn losing_the_label_clears_the_flag() {
        let (store, _b, _d, account) = held();
        store
            .apply_gmail_labels(
                account,
                &[label("m0@example.com", &["\\Inbox", "\\Starred"])],
            )
            .unwrap();
        assert_eq!(subjects(&store, &ListView::Starred), ["m0"]);

        store
            .apply_gmail_labels(account, &[label("m0@example.com", &["\\Inbox"])])
            .unwrap();
        assert!(subjects(&store, &ListView::Starred).is_empty());
    }

    /// A message we do not hold is not worth a row we could not open.
    #[test]
    fn labels_for_unknown_messages_are_ignored() {
        let (store, _b, _d, account) = held();
        let n = store
            .apply_gmail_labels(account, &[label("never-seen@example.com", &["\\Inbox"])])
            .unwrap();
        assert_eq!(n, 0);
    }
}

/// Finding one conversation when you know its id and nothing else.
///
/// The popped-out window's case: it is handed an id and cannot say which
/// mailbox the conversation is in. Looking through a view only ever found the
/// ones that happened to be in the view guessed at.
mod by_id {
    use super::*;

    #[test]
    fn finds_a_conversation_that_is_in_no_view_at_all() {
        let (store, account, ids) = seeded();
        // Archived and starred: absent from the inbox, which is where the
        // window used to look.
        let archive = store.ensure_folder(account, "archive", "archive").unwrap();
        store.place_message(ids[0], archive).unwrap();
        store.set_flags(ids[0], flags::FLAGGED, 0).unwrap();

        let thread = thread_of(&store, ids[0]);
        assert!(
            !subjects(&store, &ListView::Inbox).contains(&"m0".to_string()),
            "precondition: the conversation must not be in the inbox"
        );

        let found = store.thread_by_id(thread).unwrap();
        assert_eq!(found.map(|t| t.subject), Some("m0".to_string()));
    }

    #[test]
    fn reports_nothing_for_an_id_that_does_not_exist() {
        let (store, _account, _ids) = seeded();
        assert!(store.thread_by_id(987_654).unwrap().is_none());
    }

    #[test]
    fn agrees_with_the_listing_it_shares_a_query_with() {
        let (store, account, ids) = seeded();
        inbox_all(&store, account, &ids);
        let thread = thread_of(&store, ids[3]);
        let from_view = store
            .list_threads(&ListView::Inbox, 0, 50)
            .unwrap()
            .into_iter()
            .find(|t| t.thread_id == thread)
            .expect("in the inbox");
        let by_id = store.thread_by_id(thread).unwrap().expect("found by id");
        assert_eq!(by_id.subject, from_view.subject);
        assert_eq!(by_id.date_ms, from_view.date_ms);
        assert_eq!(by_id.message_count, from_view.message_count);
        assert_eq!(by_id.starred, from_view.starred);
    }
}

/// Archived mail files *under* Archive; the view must know that.
mod archive_tree {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::{ListView, Store};

    #[test]
    fn mail_in_archive_subfolders_is_archived_mail() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        store.ensure_folder(account, "archive", "Archive").unwrap();
        let sub = store
            .ensure_named_folder(account, "Archive/Yearly/2023")
            .unwrap();
        let unrelated = store.ensure_named_folder(account, "Archivedream").unwrap();
        let raw = |mid: &str| {
            format!(
                "From: a@example.com\r\nTo: b@example.com\r\nSubject: s\r\n\
                 Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nx\r\n"
            )
            .into_bytes()
        };
        store
            .ingest_raw(&blobs, account, Some(sub), Some(1), &raw("in-tree@x"))
            .unwrap();
        // A folder whose name merely *starts* with the archive path must not
        // be swept in — the delimiter is part of the meaning.
        store
            .ingest_raw(
                &blobs,
                account,
                Some(unrelated),
                Some(1),
                &raw("near-miss@x"),
            )
            .unwrap();

        let rows = store
            .list_threads(&ListView::parse("archive"), 0, 50)
            .unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
    }
}
