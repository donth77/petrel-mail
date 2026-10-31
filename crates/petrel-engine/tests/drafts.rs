//! Drafts: a message that outlives the composer.
//!
//! Stored as an ordinary message row with the \Draft flag, in the drafts
//! folder, so the Drafts view and every triage action work on them without
//! learning a second kind of thing.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::store::{ListView, NewMessage, Sort, SortKey, Store, flags};

fn thread_of(store: &Store, id: i64) -> i64 {
    store.thread_of(id).unwrap().unwrap_or(-id)
}

fn listed(store: &Store, role: &str) -> Vec<i64> {
    store
        .list_threads(
            &ListView::Folder(role.into()),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect()
}

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

#[test]
fn trashing_a_draft_takes_it_out_of_drafts() {
    let (s, account) = store();
    let id = s
        .save_draft(account, None, "a@example.com", "S", "b", "")
        .unwrap();
    s.apply_thread_action(
        account,
        thread_of(&s, id),
        ActionKind::Trash,
        None,
        PlacementPolicy::Exclusive,
    )
    .unwrap();

    assert!(
        listed(&s, "drafts").is_empty(),
        "a trashed draft must leave the Drafts list"
    );
    assert_eq!(listed(&s, "trash"), vec![id]);
}

#[test]
fn saving_a_trashed_draft_does_not_put_it_back_in_drafts() {
    let (s, account) = store();
    let id = s
        .save_draft(account, None, "a@example.com", "S", "one", "")
        .unwrap();
    s.apply_thread_action(
        account,
        thread_of(&s, id),
        ActionKind::Trash,
        None,
        PlacementPolicy::Exclusive,
    )
    .unwrap();
    s.save_draft(account, Some(id), "a@example.com", "S", "two", "")
        .unwrap();

    assert!(
        listed(&s, "drafts").is_empty(),
        "autosave after trash must not re-file the row in Drafts"
    );
    assert_eq!(listed(&s, "trash"), vec![id]);
    assert_eq!(s.load_draft(id).unwrap().body, "two");
}

#[test]
fn saving_a_spammed_draft_does_not_put_it_back_in_drafts() {
    let (s, account) = store();
    let id = s
        .save_draft(account, None, "a@example.com", "S", "one", "")
        .unwrap();
    s.apply_thread_action(
        account,
        thread_of(&s, id),
        ActionKind::Spam,
        None,
        PlacementPolicy::Exclusive,
    )
    .unwrap();
    s.save_draft(account, Some(id), "a@example.com", "S", "two", "")
        .unwrap();

    assert!(listed(&s, "drafts").is_empty());
    assert_eq!(listed(&s, "spam"), vec![id]);
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

    /// A search started in Drafts answers with what the Drafts list shows.
    /// Post waiting to go was found there too, so the list said nothing and
    /// the search beside it found the message (docs/24 #34).
    #[test]
    fn a_search_in_drafts_leaves_out_what_is_waiting_to_send() {
        let (s, account) = store();
        s.ensure_folder(account, "drafts", "Drafts").unwrap();
        let id = s
            .save_draft(account, None, "a@example.com", "Quarterly", "zebraword", "")
            .unwrap();
        let found = |q: &str| s.search_threads(q, 10).unwrap().len();
        assert_eq!(
            found("in:drafts zebraword"),
            1,
            "a draft is found in Drafts"
        );

        s.schedule_send(id, Some(now_ms() + 600_000)).unwrap();
        assert_eq!(
            found("in:drafts zebraword"),
            listed(&s, "drafts").len(),
            "the search agrees with the list"
        );
        assert_eq!(found("zebraword"), 1, "and is still found by its words");
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
            bcc: "board@example.org".into(),
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

    /// The staged-file sweep asks this before it deletes anything: a draft
    /// weeks old still sends what it lists, and so does one in the outbox.
    #[test]
    fn every_file_a_draft_still_lists_is_known() {
        let store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let listing = |files: &[&str]| DraftEnvelope {
            attachments: files.iter().map(|f| f.to_string()).collect(),
            ..DraftEnvelope::default()
        };
        let save = |subject: &str, envelope: &DraftEnvelope| {
            store
                .save_draft_full(account, None, "a@x", "", subject, "b", "", envelope)
                .unwrap()
        };
        save("draft", &listing(&["/data/staged/1-0/board-pack.pdf"]));
        let queued = save(
            "queued",
            &listing(&["/data/staged/2-0/minutes.pdf", "/Users/sam/notes.txt"]),
        );
        store.schedule_send(queued, Some(1_000)).unwrap();
        store
            .save_draft(account, None, "c@x", "none", "b", "")
            .unwrap();

        let mut paths = store.draft_attachment_paths().unwrap();
        paths.sort();
        assert_eq!(
            paths,
            [
                "/Users/sam/notes.txt",
                "/data/staged/1-0/board-pack.pdf",
                "/data/staged/2-0/minutes.pdf"
            ]
        );
        // Sent or discarded, it lists nothing any more.
        store.delete_draft(queued).unwrap();
        assert_eq!(
            store.draft_attachment_paths().unwrap(),
            ["/data/staged/1-0/board-pack.pdf"]
        );
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
fn drafts_list_finds_a_few_among_many_inbox_messages() {
    // The page used to walk every message looking for a drafts placement.
    // A large inbox then made opening Drafts cost the mailbox, not the
    // handful of drafts. The list must still return only those drafts.
    let (mut s, account) = store();
    let inbox = s.ensure_folder(account, "inbox", "INBOX").unwrap();
    let msgs: Vec<NewMessage> = (0..200)
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
    let ids = s.insert_messages(&msgs).unwrap();
    for id in &ids {
        s.place_message(*id, inbox).unwrap();
    }
    s.save_draft(account, None, "a@example.com", "old-draft", "one", "")
        .unwrap();
    s.save_draft(account, None, "a@example.com", "new-draft", "two", "")
        .unwrap();

    let rows = s
        .list_threads(
            &ListView::Folder("drafts".into()),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let subjects: Vec<&str> = rows.iter().map(|r| r.subject.as_str()).collect();
    assert!(subjects.contains(&"old-draft"));
    assert!(subjects.contains(&"new-draft"));
    assert!(
        !subjects.iter().any(|s| s.starts_with("inbox-")),
        "inbox mail must not leak into the drafts list"
    );
}

#[test]
fn drafts_list_pages_from_the_cursor() {
    let (s, account) = store();
    for i in 0..5 {
        s.save_draft(account, None, "a@example.com", &format!("d{i}"), "body", "")
            .unwrap();
    }
    let sort = petrel_engine::store::Sort::default();
    let first = s
        .list_threads(&ListView::Folder("drafts".into()), 0, 2, sort)
        .unwrap();
    assert_eq!(first.len(), 2);
    let rest = s
        .list_threads_after(
            &ListView::Folder("drafts".into()),
            2,
            sort,
            first[1].date_ms,
            first[1].thread_id,
        )
        .unwrap();
    assert_eq!(rest.len(), 2);
    let first_ids: Vec<i64> = first.iter().map(|r| r.id).collect();
    assert!(
        !rest.iter().any(|r| first_ids.contains(&r.id)),
        "the next page must not repeat the cursor page"
    );
}

#[test]
fn drafts_list_sorts_by_sender_among_inbox_mail() {
    // Sender sort groups first, so the FROM clause is the whole cost.
    // Starting from placements must still return only the drafts, and
    // in from-name order, not the inbox mail that shares the mailbox.
    let (mut s, account) = store();
    let inbox = s.ensure_folder(account, "inbox", "INBOX").unwrap();
    let drafts = s.ensure_folder(account, "drafts", "Drafts").unwrap();
    let inbox_msgs: Vec<NewMessage> = (0..80)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 1_000 + i,
            from_addr: "zzz@example.com".into(),
            from_display: "Zzz".into(),
            to_addr: "me@example.com".into(),
            subject: format!("inbox-{i}"),
            body_text: "body".into(),
        })
        .collect();
    let inbox_ids = s.insert_messages(&inbox_msgs).unwrap();
    for id in &inbox_ids {
        s.place_message(*id, inbox).unwrap();
    }
    for (i, name) in ["Cam", "Amy", "Ben"].iter().enumerate() {
        let ids = s
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: 20_000 + i as i64,
                from_addr: format!("{}@example.com", name.to_ascii_lowercase()),
                from_display: (*name).into(),
                to_addr: "me@example.com".into(),
                subject: format!("draft-{name}"),
                body_text: "body".into(),
            }])
            .unwrap();
        s.place_message(ids[0], drafts).unwrap();
    }

    let rows = s
        .list_threads(
            &ListView::Folder("drafts".into()),
            0,
            50,
            Sort {
                key: SortKey::Sender,
                ascending: true,
            },
        )
        .unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r.from_display.as_str()).collect();
    assert_eq!(names, ["Amy", "Ben", "Cam"]);
}

#[test]
fn drafts_keep_their_sort_once_their_copies_come_back() {
    // Drafts list message by message, keyed by `-id`. Once a draft's pushed
    // copy has been fetched back, its row names a conversation, and the page
    // was put back in order by that instead: no key matched it, so every
    // sort came back newest first (docs/24 #14).
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
    let env = petrel_engine::store::DraftEnvelope::default();
    for (uid, subject) in [(1u32, "Alpha"), (2, "Zeta")] {
        let id = store
            .save_draft_full(
                account,
                None,
                "a@example.com",
                "",
                subject,
                "words",
                "",
                &env,
            )
            .unwrap();
        let mid = format!("draft-{uid}@petrel.test");
        store.set_draft_msgid(id, &mid).unwrap();
        let copy = format!(
            "From: me@example.com\r\nTo: a@example.com\r\nSubject: {subject}\r\n\
             Date: Tue, 18 Aug 2026 14:0{uid}:00 +0000\r\nMessage-ID: <{mid}>\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nwords\r\n"
        );
        store
            .ingest_raw(&blobs, account, Some(drafts), Some(uid), copy.as_bytes())
            .unwrap();
        // Saved a moment apart, so oldest first has an answer.
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let view = ListView::parse("drafts");
    let listed = |key, ascending| -> Vec<String> {
        store
            .list_threads(&view, 0, 50, Sort { key, ascending })
            .unwrap()
            .into_iter()
            .map(|r| r.subject)
            .collect()
    };
    let rows = store.list_threads(&view, 0, 50, Sort::default()).unwrap();
    assert!(
        rows.iter().all(|r| r.thread_id > 0),
        "the copies gave both drafts a conversation"
    );
    assert_eq!(
        listed(SortKey::Date, false),
        ["Zeta", "Alpha"],
        "newest first"
    );
    assert_eq!(
        listed(SortKey::Date, true),
        ["Alpha", "Zeta"],
        "oldest first"
    );
    assert_eq!(listed(SortKey::Subject, true), ["Alpha", "Zeta"], "A to Z");
    assert_eq!(listed(SortKey::Subject, false), ["Zeta", "Alpha"], "Z to A");

    // And a page at a time, the way the window asks.
    let oldest = Sort {
        key: SortKey::Date,
        ascending: true,
    };
    let first = store.list_threads(&view, 0, 1, oldest).unwrap();
    let next = store
        .list_threads_after(&view, 1, oldest, first[0].date_ms, first[0].thread_id)
        .unwrap();
    assert_eq!(first[0].subject, "Alpha");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].subject, "Zeta");
}

#[test]
fn drafts_page_by_the_draft_both_ways_once_their_copies_come_back() {
    // Review r2, R2-1, the other half of docs/24 #14. Once a draft's copy
    // has come back it names its conversation, and two can name the same one:
    // two answers to one message. The rows keep the conversation, because the
    // marks act on it, so the window pages Drafts by what the walk keys them
    // by instead: after `(date, -id)`. Every draft, once and in order, a page
    // of one at a time, each sort both ways. Paged after the conversation,
    // newest first gave the cursor's own row again.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("p.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
    store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(1),
            b"From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\nSubject: Plans\r\n\
              Date: Tue, 18 Aug 2026 13:00:00 +0000\r\nMessage-ID: <plans@example.com>\r\n\
              MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nPlans?\r\n",
        )
        .unwrap();
    // Two answers to it, and one of its own, each pushed and come back.
    for (uid, subject, to, answers) in [
        (1u32, "Re: Plans", "dana@example.com", true),
        (2, "Re: Plans", "sam@example.com", true),
        (3, "Lunch", "alex@example.com", false),
    ] {
        let parent = answers.then(|| "<plans@example.com>".to_string());
        let env = petrel_engine::store::DraftEnvelope {
            in_reply_to: parent.clone(),
            references: parent.clone().into_iter().collect(),
            ..Default::default()
        };
        let id = store
            .save_draft_full(account, None, to, "", subject, "words", "", &env)
            .unwrap();
        let mid = format!("draft-{uid}@petrel.test");
        store.set_draft_msgid(id, &mid).unwrap();
        let threading = parent
            .map(|p| format!("In-Reply-To: {p}\r\nReferences: {p}\r\n"))
            .unwrap_or_default();
        let copy = format!(
            "From: me@example.com\r\nTo: {to}\r\nSubject: {subject}\r\n\
             Date: Tue, 18 Aug 2026 14:0{uid}:00 +0000\r\nMessage-ID: <{mid}>\r\n{threading}\
             MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nwords\r\n"
        );
        store
            .ingest_raw(&blobs, account, Some(drafts), Some(uid), copy.as_bytes())
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // And one never pushed, with no conversation at all.
    store
        .save_draft(account, None, "priya@example.com", "Notes", "words", "")
        .unwrap();

    let view = ListView::parse("drafts");
    let all = store.list_threads(&view, 0, 50, Sort::default()).unwrap();
    assert_eq!(all.len(), 4, "{all:?}");
    let answers: Vec<i64> = all
        .iter()
        .filter(|r| r.subject == "Re: Plans")
        .map(|r| r.thread_id)
        .collect();
    assert!(
        answers.len() == 2 && answers[0] > 0 && answers[0] == answers[1],
        "the two answers share a conversation, so the conversation is no key: {answers:?}"
    );

    for key in [SortKey::Date, SortKey::Subject, SortKey::Sender] {
        for ascending in [false, true] {
            let sort = Sort { key, ascending };
            let whole: Vec<i64> = store
                .list_threads(&view, 0, 50, sort)
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect();
            let mut paged = Vec::new();
            let mut page = store.list_threads(&view, 0, 1, sort).unwrap();
            while let Some(last) = page.pop() {
                paged.push(last.id);
                assert!(paged.len() <= whole.len(), "{sort:?} repeats: {paged:?}");
                // What the window sends: the row's date and the draft's key,
                // and by sender or subject what the row was listed under.
                let shown = match key {
                    SortKey::Date => None,
                    SortKey::Sender => Some(last.sort_value.clone().unwrap_or_else(|| {
                        if last.from_display.is_empty() {
                            last.from_addr.clone()
                        } else {
                            last.from_display.clone()
                        }
                    })),
                    SortKey::Subject => Some(last.sort_value.clone().unwrap_or(last.subject)),
                };
                page = store
                    .list_threads_after_shown(
                        &view,
                        1,
                        sort,
                        last.date_ms,
                        -last.id,
                        shown.as_deref(),
                    )
                    .unwrap();
            }
            assert_eq!(paged, whole, "{sort:?}");
        }
    }
}

#[test]
fn drafts_list_follows_a_vanished_sender_cursor() {
    // The cursor draft may leave the folder between pages. The fallback
    // that reads its sort key must still speak the messages table, not
    // the placements join that named it `m`.
    let (mut s, account) = store();
    let drafts = s.ensure_folder(account, "drafts", "Drafts").unwrap();
    for (i, name) in ["Amy", "Ben", "Cam", "Dan", "Eve"].iter().enumerate() {
        let ids = s
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: 10_000 + i as i64,
                from_addr: format!("{}@example.com", name.to_ascii_lowercase()),
                from_display: (*name).into(),
                to_addr: "me@example.com".into(),
                subject: format!("d{i}"),
                body_text: "body".into(),
            }])
            .unwrap();
        s.place_message(ids[0], drafts).unwrap();
    }
    let sort = Sort {
        key: SortKey::Sender,
        ascending: true,
    };
    let view = ListView::Folder("drafts".into());
    let all = s.list_threads(&view, 0, 20, sort).unwrap();
    let first = s.list_threads(&view, 0, 2, sort).unwrap();
    let cursor = first.last().unwrap().clone();
    let expected: Vec<i64> = all
        .iter()
        .skip(2)
        .map(|r| r.id)
        .filter(|id| *id != cursor.id)
        .collect();

    s.remove_placement(cursor.id, account, "Drafts").unwrap();

    let rest = s
        .list_threads_after(&view, 10, sort, cursor.date_ms, cursor.thread_id)
        .unwrap();
    let ids: Vec<i64> = rest.iter().map(|r| r.id).collect();
    assert_eq!(ids, expected);
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

/// A draft being written here, and post in the outbox, say what the composer
/// last saved — not what the copy last pushed to the server said.
///
/// The pushed copy comes back through ordinary folder sync and lands on the
/// row by Message-ID. Taking that copy's subject and recipients for the row's
/// own reverted every edit made after the push, and since a row with a send
/// time is no longer pushed again, a Send Later went out to the recipient who
/// had been removed, under the subject that had been replaced. A re-extraction
/// after an upgrade did the same from the stored copy.
mod composer_owns_its_words {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::{DraftEnvelope, ListView, Sort, Store};

    const MSGID: &str = "draft-abc@petrel.test";
    const NOW: i64 = 1_790_000_000_000;

    fn setup() -> (tempfile::TempDir, Store, BlobStore, i64, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
        (dir, store, blobs, account, drafts)
    }

    /// The copy `push_draft_to_server` renders, as folder sync fetches it.
    fn pushed_copy(to: &str, subject: &str, body: &str) -> Vec<u8> {
        format!(
            "From: Me <me@example.com>\r\nTo: {to}\r\nSubject: {subject}\r\n\
             Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <{MSGID}>\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    fn save(
        store: &Store,
        account: i64,
        id: Option<i64>,
        to: &str,
        subject: &str,
        body: &str,
    ) -> i64 {
        store
            .save_draft_full(
                account,
                id,
                to,
                "",
                subject,
                body,
                &format!("<p>{body}</p>"),
                &DraftEnvelope::default(),
            )
            .unwrap()
    }

    /// Written, pushed as uid 41 mid-composition, then finished: a
    /// recipient swapped (dana must not get this one), the subject settled.
    fn pushed_then_edited(store: &mut Store, account: i64) -> i64 {
        let id = save(
            store,
            account,
            None,
            "dana@example.com",
            "Quarterly draft",
            "first words",
        );
        store.set_draft_msgid(id, MSGID).unwrap();
        store.set_draft_server_uid(id, Some(41)).unwrap();
        save(
            store,
            account,
            Some(id),
            "sam@example.com",
            "Quarterly final numbers",
            "final words",
        );
        id
    }

    fn fetch_back_the_push(
        store: &mut Store,
        blobs: &BlobStore,
        account: i64,
        drafts: i64,
        id: i64,
    ) {
        let raw = pushed_copy("dana@example.com", "Quarterly draft", "first words");
        let ingested = store
            .ingest_raw(blobs, account, Some(drafts), Some(41), &raw)
            .unwrap();
        assert_eq!(ingested.message_id, id, "the copy still lands on the row");
        assert!(!ingested.was_new);
    }

    fn assert_as_saved(store: &Store, id: i64) {
        let rec = store.load_draft(id).unwrap();
        assert_eq!(rec.to, "sam@example.com", "a removed recipient came back");
        assert_eq!(
            rec.subject, "Quarterly final numbers",
            "a replaced subject came back"
        );
        assert_eq!(rec.body, "final words");
        let hits = |q: &str| store.search(q, 10).unwrap().len();
        assert_eq!(hits("final"), 1, "search reads what was saved");
        assert_eq!(hits("first"), 0, "and not the words the push carried");
    }

    #[test]
    fn a_send_later_goes_as_it_was_saved_after_its_push_is_fetched_back() {
        let (_d, mut store, blobs, account, drafts) = setup();
        let id = pushed_then_edited(&mut store, account);
        store.schedule_send(id, Some(NOW + 6 * 3_600_000)).unwrap();

        fetch_back_the_push(&mut store, &blobs, account, drafts, id);

        let due = store.due_sends(account, NOW + 6 * 3_600_000 + 1).unwrap();
        assert_eq!(due.len(), 1, "still queued");
        assert_eq!(due[0].to, "sam@example.com");
        assert_eq!(due[0].subject, "Quarterly final numbers");
        assert_as_saved(&store, id);
    }

    #[test]
    fn a_draft_keeps_its_later_edits_when_its_push_is_fetched_back() {
        let (_d, mut store, blobs, account, drafts) = setup();
        let id = pushed_then_edited(&mut store, account);
        fetch_back_the_push(&mut store, &blobs, account, drafts, id);
        assert_as_saved(&store, id);
        let row = store
            .list_threads(&ListView::Folder("drafts".into()), 0, 10, Sort::default())
            .unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].subject, "Quarterly final numbers");
    }

    #[test]
    fn a_fetched_back_copy_still_records_where_it_sits() {
        let (_d, mut store, blobs, account, drafts) = setup();
        let id = pushed_then_edited(&mut store, account);
        fetch_back_the_push(&mut store, &blobs, account, drafts, id);
        assert_eq!(store.folders_of(id).unwrap(), vec![drafts]);
        assert_eq!(store.placement_uids(drafts).unwrap(), vec![41]);
    }

    #[test]
    fn re_extraction_leaves_a_queued_send_as_it_was_saved() {
        let (_d, mut store, blobs, account, drafts) = setup();
        let id = save(
            &store,
            account,
            None,
            "dana@example.com",
            "Quarterly draft",
            "first words",
        );
        store.set_draft_msgid(id, MSGID).unwrap();
        store.set_draft_server_uid(id, Some(41)).unwrap();
        // Fetched back while it still said what it said, so the row holds
        // that copy's bytes; then finished and scheduled.
        fetch_back_the_push(&mut store, &blobs, account, drafts, id);
        save(
            &store,
            account,
            Some(id),
            "sam@example.com",
            "Quarterly final numbers",
            "final words",
        );
        store.schedule_send(id, Some(NOW + 86_400_000)).unwrap();

        // An update ships a new extractor overnight.
        store.set_setting("extraction_version", "9").unwrap();
        store.reindex_bodies(&blobs).unwrap();

        let due = store.due_sends(account, NOW + 86_400_001).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].to, "sam@example.com");
        assert_eq!(due[0].subject, "Quarterly final numbers");
        assert_as_saved(&store, id);
    }

    #[test]
    fn re_extraction_leaves_a_draft_as_it_was_saved() {
        let (_d, mut store, blobs, account, drafts) = setup();
        let id = save(
            &store,
            account,
            None,
            "dana@example.com",
            "Quarterly draft",
            "first words",
        );
        store.set_draft_msgid(id, MSGID).unwrap();
        fetch_back_the_push(&mut store, &blobs, account, drafts, id);
        save(
            &store,
            account,
            Some(id),
            "sam@example.com",
            "Quarterly final numbers",
            "final words",
        );
        store.set_setting("extraction_version", "9").unwrap();
        store.reindex_bodies(&blobs).unwrap();
        assert_as_saved(&store, id);
    }

    #[test]
    fn a_draft_its_copy_brings_back_is_searchable_again() {
        use petrel_engine::actions::{ActionKind, PlacementPolicy};
        // Saved and pushed; binned; deleted forever, which takes it out of
        // search with the tombstone. Then its copy lands in Trash before the
        // server has expunged it — the drain's delete racing a watched fetch —
        // and the row is live again. Live, it has to be findable.
        let (_d, mut store, blobs, account, drafts) = setup();
        let id = save(
            &store,
            account,
            None,
            "sam@example.com",
            "Quarterly final numbers",
            "final words",
        );
        store.set_draft_msgid(id, MSGID).unwrap();
        let copy = pushed_copy("sam@example.com", "Quarterly final numbers", "final words");
        store
            .ingest_raw(&blobs, account, Some(drafts), Some(41), &copy)
            .unwrap();
        let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
        let thread = store.thread_of(id).unwrap().unwrap_or(-id);
        for kind in [ActionKind::Trash, ActionKind::DeleteForever] {
            store
                .apply_thread_action(account, thread, kind, None, PlacementPolicy::Exclusive)
                .unwrap();
        }
        assert_eq!(store.search("final", 10).unwrap().len(), 0, "deleted");

        let back = store
            .ingest_raw(&blobs, account, Some(trash), Some(55), &copy)
            .unwrap();
        assert_eq!(back.message_id, id);
        assert_eq!(store.search("final", 10).unwrap().len(), 1, "by its words");
        assert_eq!(
            store.search("sam", 10).unwrap().len(),
            1,
            "by its recipient"
        );
    }

    #[test]
    fn a_draft_written_elsewhere_still_follows_its_server_copy() {
        // Nothing of it was written here: the server copy is all there is,
        // and a newer one is the draft.
        let (_d, mut store, blobs, account, drafts) = setup();
        let first = store
            .ingest_raw(
                &blobs,
                account,
                Some(drafts),
                Some(7),
                &pushed_copy("dana@example.com", "From the phone", "phone words"),
            )
            .unwrap();
        let again = store
            .ingest_raw(
                &blobs,
                account,
                Some(drafts),
                Some(8),
                &pushed_copy("sam@example.com", "From the phone, edited", "edited words"),
            )
            .unwrap();
        assert_eq!(again.message_id, first.message_id);
        let rec = store.load_draft(first.message_id).unwrap();
        assert_eq!(rec.subject, "From the phone, edited");
        assert_eq!(rec.to, "sam@example.com");
        assert_eq!(store.search("edited", 10).unwrap().len(), 1);
        assert_eq!(
            store.search("phone", 10).unwrap().len(),
            1,
            "the subject says phone"
        );
        assert_eq!(
            store.search("dana", 10).unwrap().len(),
            0,
            "the old recipient is gone from the index"
        );
    }
}

/// A draft's marker is this store's and this draft's alone, and the same
/// every time it is asked for, including after the store is closed and
/// opened again: a push removes older copies only by that sameness.
#[test]
fn a_drafts_marker_is_this_stores_and_this_drafts_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("petrel.db");
    let (first, other_draft) = {
        let store = Store::open(&path).unwrap();
        let first = store.draft_marker("d1@example.com").unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(store.draft_marker("d1@example.com").unwrap(), first);
        (first, store.draft_marker("d2@example.com").unwrap())
    };
    assert_ne!(first, other_draft, "two drafts share a marker");
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.draft_marker("d1@example.com").unwrap(),
        first,
        "the marker changed when the store was opened again"
    );
    let elsewhere = Store::open_in_memory().unwrap();
    assert_ne!(
        elsewhere.draft_marker("d1@example.com").unwrap(),
        first,
        "another store writes the same marker"
    );
}
