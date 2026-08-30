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
        .list_threads(
            &ListView::parse("tag:Urgent"),
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert!(rows.is_empty());
}

/// Inbound: a label that is a Petrel tag syncs its membership from Gmail.
mod inbound_labels {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::Store;

    fn fixture(mid: &str) -> Vec<u8> {
        format!(
            "From: a@example.com\r\nTo: b@example.com\r\nSubject: s\r\n\
             Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain\r\n\r\nbody\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn membership_follows_the_sweep_and_categories_never_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let urgent = store.ensure_tag(account, "Urgent", None).unwrap();

        let m = store
            .ingest_raw(&blobs, account, Some(inbox), Some(1), &fixture("m1@x"))
            .unwrap();

        // Gmail reports the message carrying the label that is our tag —
        // plus a system label and a stranger label, which must do nothing.
        store
            .apply_gmail_labels(
                account,
                &[(
                    "m1@x".to_string(),
                    vec![
                        "\\\\Inbox".to_string(),
                        "Urgent".to_string(),
                        "SomeFolderLabel".to_string(),
                    ],
                )],
            )
            .unwrap();
        assert_eq!(store.tags_of(m.message_id).unwrap(), vec![urgent]);
        // The stranger label did not become a tag.
        assert_eq!(store.tags_for_account(account).unwrap().len(), 1);

        // Untagged in Gmail's web UI: the tag goes here too.
        store
            .apply_gmail_labels(
                account,
                &[("m1@x".to_string(), vec!["\\\\Inbox".to_string()])],
            )
            .unwrap();
        assert!(store.tags_of(m.message_id).unwrap().is_empty());

        // A system label spelled like a tag name is never tag material.
        let inboxtag = store.ensure_tag(account, "Inbox", None).unwrap();
        store
            .apply_gmail_labels(
                account,
                &[("m1@x".to_string(), vec!["\\\\Inbox".to_string()])],
            )
            .unwrap();
        assert!(
            !store.tags_of(m.message_id).unwrap().contains(&inboxtag),
            "backslash-prefixed labels are Gmail's, not the user's"
        );
    }
}

/// The "Followup" story, start to finish.
///
/// A keyword arrives from another client, Petrel promotes it to a sidebar tag,
/// and then the thing carrying it goes away. Before the origin column, the tag
/// stayed forever: a live account grew an empty "Followup" nobody remembered
/// making. What must not happen while fixing that is losing a tag somebody
/// created themselves, which is why the last third of this test exists.
#[test]
fn a_server_keyword_that_stops_arriving_stops_being_a_tag() {
    use petrel_engine::store::Store;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let inbox = store.ensure_folder(account, "inbox", "INBOX").expect("f");
    let raw = b"From: a@example.com\r\nTo: me@example.com\r\nSubject: hi\r\n\
                Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <fu1@x>\r\n\
                MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n";
    store
        .ingest_raw(&blobs, account, Some(inbox), Some(11), raw)
        .expect("ingest");

    let names = |s: &Store| -> Vec<String> {
        s.tags_for_account(account)
            .expect("tags")
            .into_iter()
            .map(|t| t.name)
            .collect()
    };

    // Another client flags it. The keyword is not machine-shaped, so it earns
    // a sidebar entry.
    store
        .apply_keywords(account, inbox, &[(11, vec!["Followup".into()])])
        .expect("apply");
    assert!(names(&store).contains(&"Followup".to_string()));

    // A person makes one of their own, and never puts it on anything.
    store.ensure_tag(account, "Waiting on", None).expect("tag");

    // The flag is cleared elsewhere. The keyword simply stops arriving.
    store
        .apply_keywords(account, inbox, &[(11, vec![])])
        .expect("apply");

    let after = names(&store);
    assert!(
        !after.contains(&"Followup".to_string()),
        "a server tag nothing carries should not stay in the sidebar: {after:?}"
    );
    assert!(
        after.contains(&"Waiting on".to_string()),
        "an empty tag somebody made is theirs to keep: {after:?}"
    );
}

/// Applying a server-introduced tag by hand adopts it.
#[test]
fn a_server_tag_you_use_yourself_becomes_yours() {
    use petrel_engine::store::Store;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let inbox = store.ensure_folder(account, "inbox", "INBOX").expect("f");
    let raw = b"From: a@example.com\r\nTo: me@example.com\r\nSubject: hi\r\n\
                Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <fu2@x>\r\n\
                MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n";
    store
        .ingest_raw(&blobs, account, Some(inbox), Some(12), raw)
        .expect("ingest");
    store
        .apply_keywords(account, inbox, &[(12, vec!["Followup".into()])])
        .expect("apply");

    // The same name, now asked for by a person rather than found on a message.
    store.ensure_tag(account, "Followup", None).expect("adopt");

    store
        .apply_keywords(account, inbox, &[(12, vec![])])
        .expect("apply");

    let after: Vec<String> = store
        .tags_for_account(account)
        .expect("tags")
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(
        after.contains(&"Followup".to_string()),
        "once somebody uses a tag it is theirs, empty or not: {after:?}"
    );
}

/// What renaming refuses, creating must refuse too.
///
/// `rename_tag` compares names case-insensitively — "case is not a difference"
/// is the rule the test above pins. The UNIQUE(account_id, name) constraint
/// creating goes through does not: SQLite's default collation is BINARY, so
/// `Urgent` and `urgent` were two rows. That is a state the rail cannot show
/// apart, `tag:urgent` cannot pick between, and the server cannot hold at all
/// — IMAP keywords are case-insensitive, so both travel as one keyword and
/// each sync hands the pair the other's messages.
#[test]
fn a_tag_that_differs_only_in_case_is_the_same_tag() {
    let (store, account, _ids) = seeded();
    let made = store.ensure_tag(account, "Urgent", None).unwrap();
    let again = store.ensure_tag(account, "urgent", None).unwrap();

    assert_eq!(again, made, "the same tag, however it was typed");
    let names: Vec<String> = store
        .tags_for_account(account)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, vec!["Urgent".to_string()], "and only one row for it");
}

/// The same rule where the names arrive from the server rather than a person.
///
/// A keyword comes back in whatever case the server or another client felt
/// like, and each spelling used to introduce a tag of its own.
#[test]
fn a_keyword_in_another_case_does_not_introduce_a_second_tag() {
    let (store, account, _ids) = seeded();
    let mine = store.ensure_tag(account, "Waiting on", None).unwrap();
    let from_server = store.ensure_server_tag(account, "WAITING ON").unwrap();

    assert_eq!(from_server, mine);
    assert_eq!(store.tags_for_account(account).unwrap().len(), 1);
}
