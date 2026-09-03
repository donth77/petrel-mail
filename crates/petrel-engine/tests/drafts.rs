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
        .list_threads(
            &ListView::Folder("drafts".into()),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].subject, "Hello");
    // It is filed, so it must not also be sitting in the inbox.
    assert!(
        s.list_threads(
            &ListView::Inbox,
            0,
            50,
            petrel_engine::store::Sort::default()
        )
        .unwrap()
        .is_empty()
    );
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
        s.list_threads(
            &ListView::Folder("drafts".into()),
            0,
            50,
            petrel_engine::store::Sort::default()
        )
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
        s.list_threads(
            &ListView::Folder("drafts".into()),
            0,
            50,
            petrel_engine::store::Sort::default()
        )
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
            s.list_threads(
                &ListView::Folder("drafts".into()),
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .len(),
            1
        );
        assert!(
            s.list_threads(
                &ListView::Outbox,
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .is_empty()
        );

        s.schedule_send(id, Some(now_ms() + 600_000)).unwrap();

        // It is post now, not a draft — showing it in both would invite editing
        // something already on its way.
        assert!(
            s.list_threads(
                &ListView::Folder("drafts".into()),
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            s.list_threads(
                &ListView::Outbox,
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .len(),
            1
        );
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
            s.list_threads(
                &ListView::Folder("drafts".into()),
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .len(),
            1
        );
        assert!(
            s.list_threads(
                &ListView::Outbox,
                0,
                50,
                petrel_engine::store::Sort::default()
            )
            .unwrap()
            .is_empty()
        );
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
    let s = Store::open_in_memory().unwrap();
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

/// A draft is the message, once every send waits in the outbox.
///
/// It used to keep only its text. That was fine while the composer sent
/// directly and a draft was only ever a draft; it is not fine once a reply
/// waits ten seconds in the undo window and has to come out the other side
/// still threaded into its conversation and still carrying its files.
mod the_whole_message {
    use petrel_engine::store::{DraftEnvelope, Store};

    #[test]
    fn cc_headers_and_attachments_survive_the_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let envelope = DraftEnvelope {
            in_reply_to: Some("<parent@example.com>".into()),
            references: vec!["<root@example.com>".into(), "<parent@example.com>".into()],
            attachments: vec!["/tmp/board-pack.pdf".into()],
        };
        let id = store
            .save_draft_full(
                account,
                None,
                "sam@example.com, dana@example.com",
                "finance@example.com",
                "Re: Board pack",
                "As attached.",
                "<p>As attached.</p>",
                &envelope,
            )
            .unwrap();

        let back = store.load_draft(id).unwrap();
        assert_eq!(back.to, "sam@example.com, dana@example.com");
        assert_eq!(back.cc, "finance@example.com");
        assert_eq!(back.envelope, envelope);
    }

    #[test]
    fn saving_again_replaces_rather_than_accumulates() {
        // Edit a draft, remove the cc, save: the cc is gone, not doubled.
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let id = store
            .save_draft_full(
                account,
                None,
                "a@x",
                "c@x",
                "s",
                "b",
                "",
                &DraftEnvelope::default(),
            )
            .unwrap();
        store
            .save_draft_full(
                account,
                Some(id),
                "a@x",
                "",
                "s",
                "b",
                "",
                &DraftEnvelope::default(),
            )
            .unwrap();
        let back = store.load_draft(id).unwrap();
        assert_eq!(back.cc, "");
        assert_eq!(back.to, "a@x");
    }

    #[test]
    fn an_old_draft_with_no_envelope_still_loads() {
        // Rows written before the column existed hold NULL there.
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let id = store
            .save_draft(account, None, "a@x", "s", "b", "")
            .unwrap();
        let back = store.load_draft(id).unwrap();
        assert_eq!(back.envelope, DraftEnvelope::default());
        assert_eq!(back.cc, "");
    }
}

/// The continuity that makes a pushed draft an edit, not a sibling.
mod server_sync {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::{DraftEnvelope, Store};

    #[test]
    fn the_pushed_copy_comes_home_to_the_same_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();

        let id = store
            .save_draft_full(
                account,
                None,
                "dana@example.com",
                "",
                "Quarterly",
                "words",
                "<p>words</p>",
                &DraftEnvelope::default(),
            )
            .unwrap();
        // First push mints the travelling name.
        assert_eq!(store.draft_sync_state(id).unwrap(), (None, None));
        store.set_draft_msgid(id, "draft-abc@petrel.test").unwrap();
        store.set_draft_server_uid(id, Some(41)).unwrap();
        assert_eq!(
            store.draft_sync_state(id).unwrap(),
            (Some("draft-abc@petrel.test".into()), Some(41))
        );

        // The server copy, fetched back by ordinary folder sync, carries the
        // same Message-ID — and lands on the same row instead of beside it.
        let raw = "From: me@example.com\r\nTo: dana@example.com\r\nSubject: Quarterly\r\n\
             Message-ID: <draft-abc@petrel.test>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain\r\n\r\nwords\r\n";
        let ingested = store
            .ingest_raw(&blobs, account, Some(drafts), Some(41), raw.as_bytes())
            .unwrap();
        assert!(!ingested.was_new, "an edit, not a sibling");
        assert_eq!(ingested.message_id, id);
    }
}

#[test]
fn two_drafts_list_as_two_even_when_threaded_together() {
    // A draft is a thing you finish, not a conversation: the drafts view
    // lists per message, so two drafts sharing a subject — even a thread —
    // are two rows, the way the server's Drafts folder shows them.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let drafts = store
        .ensure_folder(account, "drafts", "Drafts")
        .expect("folder");
    let raw = |mid: &str, body: &str| {
        format!(
            "From: Me <me@example.com>\r\nTo: you@example.com\r\n\
             Subject: the same subject\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
             Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    };
    store
        .ingest_raw(
            &blobs,
            account,
            Some(drafts),
            Some(1),
            &raw("d1@x", "first words"),
        )
        .expect("ingest");
    store
        .ingest_raw_second_copy(
            &blobs,
            account,
            Some(drafts),
            2,
            &raw("d1@x", "second thoughts"),
        )
        .expect("second");
    let view = petrel_engine::store::ListView::parse("drafts");
    let rows = store
        .list_threads(&view, 0, 50, petrel_engine::store::Sort::default())
        .expect("list");
    assert_eq!(rows.len(), 2, "two drafts are two rows");
}

#[test]
fn a_foreign_revision_is_a_conflict_and_both_resolutions_hold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let drafts = store
        .ensure_folder(account, "drafts", "Drafts")
        .expect("folder");
    let draft = store
        .save_draft(account, None, "sam@example.com", "plans", "first words", "")
        .expect("draft");
    store.set_draft_msgid(draft, "d-abc@petrel").expect("msgid");
    store.set_draft_server_uid(draft, Some(40)).expect("uid");
    assert_eq!(store.draft_conflict(draft).expect("none yet"), None);

    // Another client's save, as the reconcile sweep stores it: a second-copy
    // row sharing the Message-ID, placed in the drafts folder.
    let raw = "From: Me <me@example.com>\r\nTo: sam@example.com\r\n\
               Subject: plans, revised\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
               Message-ID: <d-abc@petrel>\r\nMIME-Version: 1.0\r\n\
               Content-Type: text/plain; charset=utf-8\r\n\r\nsecond thoughts\r\n"
        .as_bytes()
        .to_vec();
    let other = store
        .ingest_raw_second_copy(&blobs, account, Some(drafts), 41, &raw)
        .expect("second copy");
    let found = store.draft_conflict(draft).expect("conflict");
    assert_eq!(found, Some((other.message_id, Some(41))));

    // Taking the server: its words land, its uid is recorded, the row folds.
    store
        .adopt_server_revision(draft, "plans, revised", "second thoughts", "", Some(41))
        .expect("adopt");
    store.retire_second_copy(other.message_id).expect("retire");
    let rec = store.load_draft(draft).expect("load");
    assert_eq!(rec.body, "second thoughts");
    assert_eq!(rec.subject, "plans, revised");
    assert_eq!(
        store.draft_sync_state(draft).expect("state").1,
        Some(41),
        "the adopted uid is the recorded one"
    );
    assert_eq!(store.draft_conflict(draft).expect("settled"), None);
}

/// A draft belongs to the account it was written in, whichever account the
/// rail shows by the time its server copy is pushed or dropped.
#[test]
fn a_draft_knows_its_own_account() {
    let store = Store::open_in_memory().unwrap();
    let first = store.ensure_test_account().unwrap();
    let second = store
        .add_account(
            "imap",
            "second@example.com",
            "Second",
            &petrel_engine::store::AccountServers::default(),
        )
        .unwrap();
    assert_ne!(first, second);
    let draft = store
        .save_draft(second, None, "to@example.com", "hello", "body", "")
        .unwrap();
    assert_eq!(store.account_of_message(draft).unwrap(), Some(second));
    assert_eq!(store.account_of_message(draft + 1_000_000).unwrap(), None);
}

#[test]
fn a_draft_can_be_found_by_its_words() {
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let draft = store
        .save_draft(
            account,
            None,
            "to@example.com",
            "quilt",
            "the zebra quilt is ready",
            "",
        )
        .unwrap();
    assert_eq!(store.search("zebra", 10).unwrap().len(), 1);

    store
        .save_draft(
            account,
            Some(draft),
            "to@example.com",
            "quilt",
            "the giraffe quilt is ready",
            "",
        )
        .unwrap();
    assert_eq!(
        store.search("zebra", 10).unwrap().len(),
        0,
        "an edit replaces the indexed text"
    );
    assert_eq!(store.search("giraffe", 10).unwrap().len(), 1);

    store.delete_draft(draft).unwrap();
    assert_eq!(
        store.search("giraffe", 10).unwrap().len(),
        0,
        "a sent or discarded draft leaves the index"
    );
}

/// A draft belongs to the account it was written in.
///
/// The composer follows an account switch, so a save can arrive naming the
/// account on screen rather than the one the draft was started in. The row
/// decides: its account never changes, and its placement stays in that
/// account's Drafts.
mod account_home {
    use petrel_engine::store::{DraftEnvelope, ListView, Sort, Store};

    #[test]
    fn saving_under_another_account_updates_the_row_where_it_lives() {
        let store = Store::open_in_memory().unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store.ensure_test_account().unwrap();
        let id = store
            .save_draft(
                a,
                None,
                "dana@example.com",
                "Written in A",
                "first words",
                "",
            )
            .unwrap();
        assert_eq!(store.account_of_message(id).unwrap(), Some(a));

        // The window switched to B before the autosave fired.
        let same = store
            .save_draft_full(
                b,
                Some(id),
                "dana@example.com",
                "",
                "Written in A",
                "more words",
                "",
                &DraftEnvelope::default(),
            )
            .unwrap();
        assert_eq!(same, id);
        assert_eq!(store.account_of_message(id).unwrap(), Some(a));
        assert_eq!(store.load_draft(id).unwrap().body, "more words");

        // Listed under A, and only under A.
        store.set_active_account(a).unwrap();
        let in_a = store
            .list_threads(&ListView::Folder("drafts".into()), 0, 10, Sort::default())
            .unwrap();
        assert_eq!(in_a.len(), 1);
        store.set_active_account(b).unwrap();
        let in_b = store
            .list_threads(&ListView::Folder("drafts".into()), 0, 10, Sort::default())
            .unwrap();
        assert!(in_b.is_empty(), "B has no draft: {in_b:?}");
        assert!(
            store.folder_for_role(b, "drafts").unwrap().is_none(),
            "nothing was filed in B, so B needed no Drafts folder"
        );
    }
}
