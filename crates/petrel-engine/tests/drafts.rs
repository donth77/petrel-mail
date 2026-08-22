//! Drafts: a message that outlives the composer.
//!
//! Stored as an ordinary message row with the \Draft flag, in the drafts
//! folder, so the Drafts view and every triage action work on them without
//! learning a second kind of thing.

use petrel_engine::store::{ListView, Store, flags};

fn store() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let account = s.ensure_test_account().unwrap();
    (s, account)
}

#[test]
fn a_saved_draft_appears_in_the_drafts_view_and_nowhere_else() {
    let (s, account) = store();
    s.save_draft(account, None, "sam@example.com", "Hello", "Body text", "")
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
        .save_draft(account, None, "sam@example.com", "Long", &body, "")
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
        .save_draft(account, None, "a@example.com", "First", "one", "")
        .unwrap();
    let same = s
        .save_draft(account, Some(id), "b@example.com", "Second", "two", "")
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
        .save_draft(account, None, "a@example.com", "S", "b", "")
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
        .save_draft(account, None, "a@example.com", "S", "b", "")
        .unwrap();
    s.delete_draft(id).unwrap();
    assert!(
        s.list_threads(&ListView::Folder("drafts".into()), 0, 50)
            .unwrap()
            .is_empty()
    );
}

/// Send later: a draft with a time on it.
mod outbox {
    use super::*;

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    #[test]
    fn scheduling_moves_a_draft_from_drafts_to_the_outbox() {
        let (s, account) = store();
        let id = s
            .save_draft(account, None, "a@example.com", "Later", "body", "")
            .unwrap();
        assert_eq!(
            s.list_threads(&ListView::Folder("drafts".into()), 0, 50)
                .unwrap()
                .len(),
            1
        );
        assert!(s.list_threads(&ListView::Outbox, 0, 50).unwrap().is_empty());

        s.schedule_send(id, Some(now_ms() + 600_000)).unwrap();

        // It is post now, not a draft — showing it in both would invite editing
        // something already on its way.
        assert!(
            s.list_threads(&ListView::Folder("drafts".into()), 0, 50)
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.list_threads(&ListView::Outbox, 0, 50).unwrap().len(), 1);
    }

    #[test]
    fn a_scheduled_send_can_be_pulled_back() {
        let (s, account) = store();
        let id = s
            .save_draft(account, None, "a@example.com", "Later", "body", "")
            .unwrap();
        s.schedule_send(id, Some(now_ms() + 600_000)).unwrap();
        s.schedule_send(id, None).unwrap();

        // An outbox you cannot retrieve something from is a worse promise than
        // sending at once.
        assert_eq!(
            s.list_threads(&ListView::Folder("drafts".into()), 0, 50)
                .unwrap()
                .len(),
            1
        );
        assert!(s.list_threads(&ListView::Outbox, 0, 50).unwrap().is_empty());
    }

    #[test]
    fn only_messages_whose_time_has_come_are_due() {
        let (s, account) = store();
        let soon = s
            .save_draft(account, None, "a@example.com", "Soon", "b", "")
            .unwrap();
        let later = s
            .save_draft(account, None, "b@example.com", "Later", "b", "")
            .unwrap();
        s.schedule_send(soon, Some(now_ms() - 1_000)).unwrap();
        s.schedule_send(later, Some(now_ms() + 600_000)).unwrap();

        let due = s.due_sends(account, now_ms()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].subject, "Soon");
    }

    #[test]
    fn one_missed_while_the_app_was_shut_is_still_due() {
        let (s, account) = store();
        let id = s
            .save_draft(account, None, "a@example.com", "Yesterday", "b", "")
            .unwrap();
        // Due long ago. Nothing ran in between, and nothing needed to.
        s.schedule_send(id, Some(now_ms() - 86_400_000)).unwrap();
        assert_eq!(s.due_sends(account, now_ms()).unwrap().len(), 1);
    }
}

/// A draft keeps both halves, because the one that sends it may have no editor.
///
/// A scheduled message goes out hours later from a background pass; there is
/// nothing to ask for the rich text then. Deriving it back from stored text
/// would flatten the message the user actually wrote.
#[test]
fn a_draft_remembers_its_formatting() {
    let mut s = Store::open_in_memory().unwrap();
    let account = s.ensure_test_account().unwrap();
    let id = s
        .save_draft(
            account,
            None,
            "sam@example.com",
            "Plan",
            "Here is the plan <https://x.example/p>.",
            r#"<p>Here is the <a href="https://x.example/p">plan</a>.</p>"#,
        )
        .unwrap();

    let back = s.load_draft(id).unwrap();
    assert!(
        back.body.contains("<https://x.example/p>"),
        "text half lost"
    );
    assert!(back.html.contains("<a href="), "rich half lost");

    // And a draft written before there was a rich half reads back empty rather
    // than failing, so old drafts still open.
    let plain = s
        .save_draft(account, None, "a@example.com", "Old", "just words", "")
        .unwrap();
    assert_eq!(s.load_draft(plain).unwrap().html, "");
}
