//! Rules: stored, ordered, and edited as a list whose order is the run order.

use petrel_engine::rules::{Actions, Condition};
use petrel_engine::store::Store;

fn setup() -> (tempfile::TempDir, Store, i64) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db")).unwrap();
    let account = store.ensure_test_account().unwrap();
    (dir, store, account)
}

fn cond(field: &str, contains: &str) -> Condition {
    Condition {
        field: field.into(),
        contains: contains.into(),
    }
}

#[test]
fn rules_keep_their_order_and_their_edits() {
    let (_dir, mut store, account) = setup();
    let a = store
        .save_rule(
            account,
            None,
            "first",
            true,
            &[cond("from", "a@x")],
            &Actions::default(),
        )
        .unwrap();
    let b = store
        .save_rule(
            account,
            None,
            "second",
            true,
            &[cond("subject", "hi")],
            &Actions::default(),
        )
        .unwrap();

    let rules = store.rules_for_account(account).unwrap();
    assert_eq!(
        rules.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a, b],
        "new rules land at the end"
    );

    // Reorder: the second runs first now.
    store.move_rule(b, true).unwrap();
    let rules = store.rules_for_account(account).unwrap();
    assert_eq!(rules.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b, a]);
    // Moving the top one up is a no-op, not an error.
    store.move_rule(b, true).unwrap();
    assert_eq!(store.rules_for_account(account).unwrap()[0].id, b);

    // Edit in place: same id, new substance.
    store
        .save_rule(
            account,
            Some(a),
            "first, renamed",
            false,
            &[cond("list_id", "news")],
            &Actions {
                skip_inbox: true,
                ..Actions::default()
            },
        )
        .unwrap();
    let rules = store.rules_for_account(account).unwrap();
    let edited = rules.iter().find(|r| r.id == a).unwrap();
    assert_eq!(edited.name, "first, renamed");
    assert!(!edited.enabled);
    assert!(edited.actions.skip_inbox);
    assert_eq!(edited.conditions[0].field, "list_id");

    store.delete_rule(b).unwrap();
    assert_eq!(store.rules_for_account(account).unwrap().len(), 1);
}

#[test]
fn keywords_come_home_as_the_tags_that_sent_them() {
    use petrel_engine::keywords::tag_keyword;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let inbox = store
        .ensure_folder(account, "inbox", "INBOX")
        .expect("folder");
    let raw = b"From: a@example.com\r\nTo: me@example.com\r\nSubject: hi\r\n\
                Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <k1@x>\r\n\
                MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n";
    let m = store
        .ingest_raw(&blobs, account, Some(inbox), Some(7), raw)
        .expect("ingest");
    // A tag whose name does not survive as an atom: it travels munged, and
    // must come home as itself rather than as a second tag.
    let waiting = store.ensure_tag(account, "Waiting on", None).expect("tag");
    assert_eq!(tag_keyword("Waiting on"), "Waiting_on");

    // The server says this message wears that keyword, and one nobody here
    // has ever heard of.
    let changed = store
        .apply_keywords(
            account,
            inbox,
            &[(7, vec!["Waiting_on".into(), "FromPhone".into()])],
        )
        .expect("apply");
    assert_eq!(changed, 2);
    let by_id: std::collections::HashMap<i64, String> = store
        .tags_for_account(account)
        .expect("all tags")
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let names = |store: &petrel_engine::store::Store| -> Vec<String> {
        let mut v: Vec<String> = store
            .tags_of(m.message_id)
            .expect("tags")
            .into_iter()
            .filter_map(|id| by_id.get(&id).cloned())
            .collect();
        v.sort();
        v
    };
    let names_now = names(&store);
    assert!(
        names_now.contains(&"Waiting on".to_string()),
        "{names_now:?}"
    );
    assert!(
        names_now.contains(&"FromPhone".to_string()),
        "{names_now:?}"
    );
    assert_eq!(
        store.tags_for_account(account).expect("all").len(),
        2,
        "the munged keyword matched its tag rather than making a new one"
    );
    let _ = waiting;

    // Untagged elsewhere: the server's word removes it here too.
    let changed = store
        .apply_keywords(account, inbox, &[(7, vec!["FromPhone".into()])])
        .expect("apply again");
    assert_eq!(changed, 1);
    assert_eq!(names(&store), vec!["FromPhone".to_string()]);

    // Idempotent: saying the same thing twice changes nothing.
    assert_eq!(
        store
            .apply_keywords(account, inbox, &[(7, vec!["FromPhone".into()])])
            .expect("third"),
        0
    );
}
