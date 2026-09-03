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
        header: None,
        op: petrel_engine::rules::Op::Contains,
        value: contains.into(),
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

/// The bug: a rule that files mail into a folder the user has since deleted
/// made the mail disappear.
///
/// A move clears every placement and then files the message at the
/// destination, with no transaction around the pair. Against a folder that no
/// longer exists the second half failed on the foreign key with the first half
/// already committed, and the message was left placed nowhere at all — gone
/// from the inbox, gone from the folder, gone from every view, and not in the
/// trash either. Proven against a real store before the fix: placements went
/// from [inbox] to [].
///
/// A rule keeps naming a folder long after the folder has gone, so it did this
/// silently to every message it matched, once per arrival.
#[test]
fn an_action_naming_a_deleted_folder_leaves_the_mail_where_it_was() {
    use petrel_engine::actions::ActionKind;
    use petrel_engine::blob::BlobStore;

    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let marketing = store.ensure_named_folder(account, "Marketing").unwrap();
    let tag = store.ensure_tag(account, "Invoices", None).unwrap();
    let raw = b"From: Dana Wu <dana@vendorco.example>\r\nTo: me@example.com\r\n\
                Subject: Q3 Invoice\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
                Message-ID: <inv1@x>\r\nMIME-Version: 1.0\r\n\
                Content-Type: text/plain\r\n\r\nbody\r\n";
    let m = store
        .ingest_raw(&blobs, account, Some(inbox), Some(1), raw)
        .unwrap();
    let policy = store.placement_policy(account).unwrap();
    let thread = store.thread_of(m.message_id).unwrap().unwrap();

    // The rule still says "move to Marketing, tag Invoices". Both are gone.
    store.remove_folder(marketing).unwrap();
    store.delete_tag(tag).unwrap();

    for (kind, target) in [
        (ActionKind::Move, marketing),
        (ActionKind::Tag, tag),
        (ActionKind::Untag, tag),
    ] {
        let err = store
            .apply_thread_action(account, thread, kind, Some(target), policy)
            .expect_err("a target that is gone is refused, not attempted");
        assert!(err.to_string().contains("does not have"), "{kind:?}: {err}");
        assert_eq!(
            store.folders_of(m.message_id).unwrap(),
            vec![inbox],
            "{kind:?} left the message where it was"
        );
    }

    // Nothing was queued for a server that would have been told nonsense.
    assert!(store.pending_actions(account).unwrap().is_empty());
}

/// The other way a target can be wrong: it exists, but next door.
///
/// A folder id from another account is the deleted case wearing a plausible
/// disguise — the insert succeeds, and the mail is quietly filed in a folder
/// belonging to a different address. Nothing offers such an id today, because
/// every folder and tag list is scoped to the account already. This is the
/// floor under that: the day something offers one account's folders while
/// another is on screen, it is a refused action rather than mail that has
/// wandered off.
#[test]
fn an_action_naming_another_accounts_folder_is_refused() {
    use petrel_engine::actions::ActionKind;
    use petrel_engine::blob::BlobStore;

    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let mine = store.ensure_test_account().unwrap();
    let theirs = store
        .add_account(
            "imap",
            "other@example.com",
            "Other",
            &petrel_engine::store::AccountServers::default(),
        )
        .unwrap();

    let inbox = store.ensure_folder(mine, "inbox", "INBOX").unwrap();
    let next_door = store.ensure_named_folder(theirs, "Theirs").unwrap();
    let their_tag = store.ensure_tag(theirs, "Theirs", None).unwrap();
    let raw = b"From: Dana Wu <dana@vendorco.example>\r\nTo: me@example.com\r\n\
                Subject: Q3 Invoice\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
                Message-ID: <inv2@x>\r\nMIME-Version: 1.0\r\n\
                Content-Type: text/plain\r\n\r\nbody\r\n";
    let m = store
        .ingest_raw(&blobs, mine, Some(inbox), Some(1), raw)
        .unwrap();
    let policy = store.placement_policy(mine).unwrap();
    let thread = store.thread_of(m.message_id).unwrap().unwrap();

    for (kind, target) in [(ActionKind::Move, next_door), (ActionKind::Tag, their_tag)] {
        store
            .apply_thread_action(mine, thread, kind, Some(target), policy)
            .expect_err("one account cannot file into another's");
        assert_eq!(
            store.folders_of(m.message_id).unwrap(),
            vec![inbox],
            "{kind:?} left the message where it was"
        );
    }
    assert!(store.pending_actions(mine).unwrap().is_empty());
}

/// The fields and operators a rule can be written in.
///
/// Rules began as "does this field contain this text", four fields and one
/// test. That answers "mail from Dana" and almost nothing else: not "from
/// exactly this address rather than anyone whose name contains it", not
/// "everything except the daily digest", not "anything over ten megabytes".
mod conditions {
    use petrel_engine::rules::{Actions, Condition, Envelope, Op, Rule, matches};

    fn envelope() -> Envelope {
        Envelope {
            from: "dana wu <dana@vendorco.example>".into(),
            to: "me@example.com".into(),
            cc: "priya nair <priya@vendorco.example>".into(),
            from_parts: vec!["dana wu".into(), "dana@vendorco.example".into()],
            to_parts: vec!["me@example.com".into()],
            cc_parts: vec!["priya nair".into(), "priya@vendorco.example".into()],
            subject: "q3 invoice attached".into(),
            list_id: String::new(),
            body: "please find the invoice for september attached.".into(),
            size: 2_400_000,
            // 2026-08-30T00:00:00Z
            date_ms: 1_788_048_000_000,
            headers: vec![
                ("x-spam-flag".into(), "NO".into()),
                ("precedence".into(), "bulk".into()),
                ("received".into(), "from a.example".into()),
                ("received".into(), "from b.example".into()),
            ],
        }
    }

    fn rule(field: &str, op: Op, value: &str) -> Rule {
        named_rule(field, None, op, value)
    }

    fn named_rule(field: &str, header: Option<&str>, op: Op, value: &str) -> Rule {
        Rule {
            id: 1,
            position: 0,
            enabled: true,
            name: "r".into(),
            conditions: vec![Condition {
                field: field.into(),
                header: header.map(str::to_string),
                op,
                value: value.into(),
            }],
            actions: Actions::default(),
        }
    }

    fn hit(field: &str, op: Op, value: &str) -> bool {
        matches(&rule(field, op, value), &envelope())
    }

    #[test]
    fn every_text_operator_and_its_opposite() {
        assert!(hit("subject", Op::Contains, "invoice"));
        assert!(!hit("subject", Op::NotContains, "invoice"));
        assert!(hit("subject", Op::Is, "Q3 Invoice Attached"));
        assert!(!hit("subject", Op::Is, "invoice"));
        assert!(hit("subject", Op::IsNot, "invoice"));
        assert!(hit("subject", Op::StartsWith, "q3"));
        assert!(!hit("subject", Op::StartsWith, "invoice"));
        assert!(hit("subject", Op::NotStartsWith, "invoice"));
        assert!(hit("subject", Op::EndsWith, "attached"));
        assert!(hit("subject", Op::NotEndsWith, "q3"));
    }

    #[test]
    fn the_new_fields_see_what_they_should() {
        assert!(hit("cc", Op::Contains, "priya"));
        assert!(hit("body", Op::Contains, "september"));
        // Cc is its own field, not quietly folded into To.
        assert!(!hit("to", Op::Contains, "priya"));
    }

    #[test]
    fn size_is_in_kilobytes_because_that_is_what_gets_typed() {
        assert!(hit("size", Op::Over, "1000"));
        assert!(!hit("size", Op::Over, "5000"));
        assert!(hit("size", Op::Under, "5000"));
        // A number that is not a number matches nothing rather than zero.
        assert!(!hit("size", Op::Over, "big"));
    }

    #[test]
    fn dates_compare_by_whole_days() {
        assert!(hit("date", Op::Before, "2026-09-01"));
        assert!(
            !hit("date", Op::Before, "2026-08-30"),
            "its own day is not before itself"
        );
        assert!(hit("date", Op::After, "2026-08-29"));
        assert!(
            !hit("date", Op::After, "2026-08-30"),
            "its own day is not after itself"
        );
        assert!(!hit("date", Op::Before, "not-a-date"));
    }

    #[test]
    fn a_header_condition_names_its_header() {
        let env = envelope();
        assert!(matches(
            &named_rule("header", Some("X-Spam-Flag"), Op::Is, "no"),
            &env
        ));
        assert!(!matches(
            &named_rule("header", Some("X-Spam-Flag"), Op::Is, "yes"),
            &env
        ));
        // A header that is not there does not contain anything — so the
        // positive test fails and the negative one holds.
        assert!(!matches(
            &named_rule("header", Some("X-Nope"), Op::Contains, "x"),
            &env
        ));
        assert!(matches(
            &named_rule("header", Some("X-Nope"), Op::NotContains, "x"),
            &env
        ));
        // Naming no header at all matches nothing.
        assert!(!matches(&rule("header", Op::Contains, "x"), &env));
    }

    #[test]
    fn a_repeated_header_means_any_of_them_and_all_of_them() {
        let env = envelope();
        // Positive: any copy will do.
        assert!(matches(
            &named_rule("header", Some("Received"), Op::Contains, "b.example"),
            &env
        ));
        // Negative: every copy has to hold, or a message slips through on the
        // strength of the innocent one.
        assert!(!matches(
            &named_rule("header", Some("Received"), Op::NotContains, "b.example"),
            &env
        ));
        assert!(matches(
            &named_rule("header", Some("Received"), Op::NotContains, "c.example"),
            &env
        ));
    }

    #[test]
    fn a_half_written_rule_matches_nothing() {
        // Whichever way it is phrased. "Does not contain nothing" is true of
        // every message ever sent, and a rule being typed must not fire.
        for op in [Op::Contains, Op::NotContains, Op::Is, Op::IsNot] {
            assert!(!hit("subject", op, ""), "empty value matched with {op:?}");
        }
    }

    #[test]
    fn a_rule_written_before_operators_existed_still_means_what_it_meant() {
        // The shape on disk from the days when substring was the only test.
        let old: Condition =
            serde_json::from_str(r#"{"field":"from","contains":"vendorco"}"#).unwrap();
        assert_eq!(old.op, Op::Contains);
        assert_eq!(old.value, "vendorco");
        assert!(matches(
            &Rule {
                id: 1,
                position: 0,
                enabled: true,
                name: "r".into(),
                conditions: vec![old],
                actions: Actions::default(),
            },
            &envelope()
        ));
    }
}

/// The whole chain, from bytes off the wire to a verdict.
///
/// The condition tests build an `Envelope` by hand, which proves the matching
/// but assumes the envelope is filled correctly. That assumption is where a
/// rules feature quietly dies: every unit test passes and no rule ever fires,
/// because the field the matcher reads is not the field the parser wrote.
mod end_to_end {
    use petrel_engine::rules::{Actions, Condition, Envelope, Op, Rule, matches};

    const RAW: &[u8] = b"From: Dana Wu <dana@vendorco.example>\r\n\
To: Tom <me@example.com>\r\n\
Cc: Priya Nair <priya@vendorco.example>\r\n\
Subject: Q3 invoice attached\r\n\
Date: Sun, 30 Aug 2026 09:00:00 +0000\r\n\
X-Spam-Flag: NO\r\n\
Received: from a.example\r\n\
Received: from b.example\r\n\
\r\n\
Please find the September invoice attached.\r\n";

    fn envelope() -> Envelope {
        let parsed = petrel_mime::parse_message(RAW).expect("parses");
        Envelope::from_message(&parsed, RAW.len() as u64)
    }

    fn rule(field: &str, header: Option<&str>, op: Op, value: &str) -> Rule {
        Rule {
            id: 1,
            position: 0,
            enabled: true,
            name: "r".into(),
            conditions: vec![Condition {
                field: field.into(),
                header: header.map(str::to_string),
                op,
                value: value.into(),
            }],
            actions: Actions::default(),
        }
    }

    fn hit(field: &str, op: Op, value: &str) -> bool {
        matches(&rule(field, None, op, value), &envelope())
    }

    #[test]
    fn a_real_message_fills_every_field_a_rule_can_read() {
        assert!(hit("from", Op::Contains, "vendorco"), "from");
        assert!(hit("from", Op::Contains, "dana wu"), "display name too");
        assert!(hit("to", Op::Contains, "me@example.com"), "to");
        assert!(hit("cc", Op::Contains, "priya"), "cc");
        assert!(hit("subject", Op::StartsWith, "q3"), "subject");
        assert!(hit("body", Op::Contains, "september"), "body");
        assert!(hit("date", Op::After, "2026-08-29"), "date");
        assert!(hit("size", Op::Over, "0"), "size");
    }

    #[test]
    fn a_header_rule_reads_the_value_not_the_whole_line() {
        // The parser slices header values out of the raw bytes. Slicing from
        // the wrong offset would give "X-Spam-Flag: NO" here, and a rule
        // written as `is NO` would never match while looking perfectly sane.
        let env = envelope();
        assert!(matches(
            &rule("header", Some("X-Spam-Flag"), Op::Is, "NO"),
            &env
        ));
        assert!(!matches(
            &rule("header", Some("X-Spam-Flag"), Op::Contains, "x-spam-flag"),
            &env
        ));
    }

    #[test]
    fn both_copies_of_a_repeated_header_survive_the_parser() {
        let env = envelope();
        assert!(matches(
            &rule("header", Some("Received"), Op::Contains, "a.example"),
            &env
        ));
        assert!(matches(
            &rule("header", Some("Received"), Op::Contains, "b.example"),
            &env
        ));
    }

    #[test]
    fn the_negatives_hold_on_a_real_message() {
        assert!(hit("subject", Op::NotContains, "refund"));
        assert!(!hit("subject", Op::NotContains, "invoice"));
        assert!(hit("from", Op::NotEndsWith, "@gmail.com"));
    }
}

/// A rule has to survive being put down and picked up again.
///
/// Conditions are stored as JSON, so the operator and the header name only
/// exist on disk if serde writes them and reads them back. A field that
/// silently defaults on load is the failure that looks like the rule
/// "stopped working" weeks later, with nothing to point at.
mod round_trip {
    use petrel_engine::rules::{Actions, Condition, Op};
    use petrel_engine::store::Store;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Store, i64) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let account = store.ensure_test_account().unwrap();
        (dir, store, account)
    }

    #[test]
    fn every_operator_and_a_header_name_come_back_unchanged() {
        let (_dir, mut store, account) = setup();
        let conditions: Vec<Condition> = [
            ("from", None, Op::Contains, "a"),
            ("to", None, Op::NotContains, "b"),
            ("cc", None, Op::Is, "c"),
            ("subject", None, Op::IsNot, "d"),
            ("body", None, Op::StartsWith, "e"),
            ("list_id", None, Op::NotStartsWith, "f"),
            ("header", Some("X-Spam-Flag"), Op::EndsWith, "g"),
            ("header", Some("Precedence"), Op::NotEndsWith, "h"),
            ("size", None, Op::Over, "100"),
            ("date", None, Op::Before, "2026-01-01"),
        ]
        .into_iter()
        .map(|(field, header, op, value)| Condition {
            field: field.into(),
            header: header.map(str::to_string),
            op,
            value: value.into(),
        })
        .collect();

        let id = store
            .save_rule(
                account,
                None,
                "everything",
                true,
                &conditions,
                &Actions::default(),
            )
            .unwrap();

        let back = store.rules_for_account(account).unwrap();
        let saved = back.iter().find(|r| r.id == id).expect("the rule is there");
        assert_eq!(
            saved.conditions, conditions,
            "a condition changed on the way to disk and back"
        );
    }
}

/// The exact operators, asked of an address rather than of the rendered line.
///
/// "From is dana@vendorco.example" is the rule somebody writes to pin one
/// sender. Compared against "dana wu dana@vendorco.example" it could never
/// hold — nor could `starts with` or `ends with`, on From, To or Cc, for any
/// message ever — while the rule sat in the list looking enabled.
mod exact_addresses {
    use petrel_engine::rules::{Actions, Condition, Envelope, Op, Rule, matches};

    const RAW: &[u8] = b"From: Dana Wu <dana@vendorco.example>\r\n\
        To: Sam Okafor <sam@example.com>, billing@example.com\r\n\
        Cc: Ada Chen <ada@example.com>\r\n\
        Subject: Q3 Invoice attached\r\n\
        Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
        Message-ID: <inv1@x>\r\nMIME-Version: 1.0\r\n\
        Content-Type: text/plain\r\n\r\nbody\r\n";

    fn envelope() -> Envelope {
        let parsed = petrel_mime::parse_message(RAW).expect("parses");
        Envelope::from_message(&parsed, RAW.len() as u64)
    }

    fn hit(field: &str, op: Op, value: &str) -> bool {
        let rule = Rule {
            id: 1,
            position: 0,
            enabled: true,
            name: "r".into(),
            conditions: vec![Condition {
                field: field.into(),
                header: None,
                op,
                value: value.into(),
            }],
            actions: Actions::default(),
        };
        matches(&rule, &envelope())
    }

    #[test]
    fn is_matches_an_address_and_a_display_name() {
        assert!(hit("from", Op::Is, "dana@vendorco.example"));
        assert!(hit("from", Op::Is, "Dana Wu"), "the name counts too");
        assert!(!hit("from", Op::Is, "dana"));
        assert!(hit("to", Op::Is, "sam@example.com"));
        assert!(hit("to", Op::Is, "billing@example.com"), "any of them");
        assert!(hit("cc", Op::Is, "ada@example.com"));
        assert!(!hit("cc", Op::Is, "sam@example.com"));
    }

    #[test]
    fn starts_and_ends_with_read_one_address_at_a_time() {
        assert!(hit("from", Op::StartsWith, "dana@"));
        assert!(hit("from", Op::EndsWith, "@vendorco.example"));
        assert!(
            hit("to", Op::StartsWith, "billing@"),
            "the second recipient"
        );
        assert!(
            hit("to", Op::EndsWith, "@example.com"),
            "shared by both, and true of each"
        );
        assert!(!hit("to", Op::StartsWith, "dana@"));
    }

    #[test]
    fn a_negative_test_has_to_hold_for_every_address() {
        // One recipient is billing@, so "is not billing@" is false for the
        // line however innocent the other recipient looks.
        assert!(!hit("to", Op::IsNot, "billing@example.com"));
        assert!(hit("to", Op::IsNot, "someone-else@example.com"));
        assert!(!hit("from", Op::NotEndsWith, "@vendorco.example"));
        assert!(hit("from", Op::NotEndsWith, "@example.com"));
        assert!(!hit("to", Op::NotStartsWith, "sam@"));
    }

    #[test]
    fn contains_still_reads_the_whole_line() {
        assert!(hit("from", Op::Contains, "vendorco"));
        assert!(hit("to", Op::Contains, "Sam Okafor"));
        assert!(!hit("to", Op::Contains, "Ada"), "cc is not to");
        assert!(hit("to", Op::NotContains, "dana"));
    }

    #[test]
    fn a_half_written_rule_still_matches_nothing() {
        for op in [
            Op::Is,
            Op::IsNot,
            Op::StartsWith,
            Op::NotStartsWith,
            Op::EndsWith,
            Op::NotEndsWith,
        ] {
            assert!(!hit("from", op, ""), "empty value matched with {op:?}");
            assert!(!hit("to", op, ""), "empty value matched with {op:?}");
        }
    }
}
