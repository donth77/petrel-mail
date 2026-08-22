//! Drafts: a message that outlives the composer.
//!
//! Stored as an ordinary message row with the \Draft flag, in the drafts
//! folder, so the Drafts view and every triage action work on them without
//! learning a second kind of thing.

use petrel_engine::store::{ListView, Store, flags};

fn store() -> (Store, i64) {
    let mut s = Store::open_in_memory().unwrap();
    let account = s.ensure_test_account().unwrap();
    (s, account)
}

#[test]
fn a_saved_draft_appears_in_the_drafts_view_and_nowhere_else() {
    let (s, account) = store();
    s.save_draft(account, None, "sam@example.com", "Hello", "Body text")
        .unwrap();

    let drafts = s
        .list_threads(&ListView::Folder("drafts".into()), 0, 50)
        .unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].subject, "Hello");
    // It is filed, so it must not also be sitting in the inbox.
    assert!(s.list_threads(&ListView::Inbox, 0, 50).unwrap().is_empty());
}

#[test]
fn a_draft_comes_back_with_every_word_that_was_written() {
    let (s, account) = store();
    // Longer than the snippet, which is where a truncated column would show.
    let body = "x".repeat(5_000);
    let id = s
        .save_draft(account, None, "sam@example.com", "Long", &body)
        .unwrap();

    let back = s.load_draft(id).unwrap();
    assert_eq!(
        back.body.len(),
        5_000,
        "the body was truncated on the way back"
    );
    assert_eq!(back.to, "sam@example.com");
    assert_eq!(back.subject, "Long");
}

#[test]
fn saving_again_updates_rather_than_multiplying() {
    let (s, account) = store();
    let id = s
        .save_draft(account, None, "a@example.com", "First", "one")
        .unwrap();
    let same = s
        .save_draft(account, Some(id), "b@example.com", "Second", "two")
        .unwrap();

    assert_eq!(id, same);
    assert_eq!(
        s.list_threads(&ListView::Folder("drafts".into()), 0, 50)
            .unwrap()
            .len(),
        1
    );
    let back = s.load_draft(id).unwrap();
    assert_eq!(back.subject, "Second");
    assert_eq!(back.body, "two");
    // Recipients are replaced, not appended: editing "to" must not leave the
    // old address attached and quietly send to both.
    assert_eq!(back.to, "b@example.com");
}

#[test]
fn a_draft_is_marked_as_one_and_counts_as_read() {
    let (s, account) = store();
    let id = s
        .save_draft(account, None, "a@example.com", "S", "b")
        .unwrap();
    let f = s.flags_of(id).unwrap();
    assert!(f & flags::DRAFT != 0, "not flagged as a draft");
    // Your own unfinished message is not unread mail; counting it would inflate
    // the badge every time someone starts typing.
    assert!(f & flags::SEEN != 0, "a draft should not count as unread");
}

#[test]
fn deleting_a_draft_removes_it() {
    let (s, account) = store();
    let id = s
        .save_draft(account, None, "a@example.com", "S", "b")
        .unwrap();
    s.delete_draft(id).unwrap();
    assert!(
        s.list_threads(&ListView::Folder("drafts".into()), 0, 50)
            .unwrap()
            .is_empty()
    );
}
