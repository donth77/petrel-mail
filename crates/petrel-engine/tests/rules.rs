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
