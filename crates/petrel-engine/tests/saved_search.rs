//! Saved searches: a question, pinned under a name (docs 22).
//!
//! What is asserted here is that the store keeps the query exactly as it was
//! typed, keeps the order the user gave, and forgets a search when the account
//! it belonged to goes. Whether a query finds the right mail is
//! `search_grammar.rs`'s business; a saved search adds no searching of its own.

use petrel_engine::search_query::Renamed;
use petrel_engine::store::Store;

fn store() -> (Store, i64, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db")).unwrap();
    let account = store.ensure_test_account().unwrap();
    (store, account, dir)
}

#[test]
fn a_search_comes_back_exactly_as_it_was_typed() {
    let (mut store, account, _dir) = store();
    // Spacing, capitals and brackets all survive: what goes back into the
    // field has to be what was in it, or opening a saved search would quietly
    // rewrite it into a canonical spelling nobody chose.
    let typed = "(from:sam OR from:dana)  invoice NOT draft";
    let id = store
        .create_saved_search(account, "  Invoices  ", typed)
        .unwrap();

    let saved = store.saved_search(id).unwrap().expect("just made it");
    assert_eq!(saved.query, typed);
    // The name is trimmed, because a name with edges is a name that looks
    // misaligned in a sidebar; the query is not, because its spaces are
    // somebody's query.
    assert_eq!(saved.name, "Invoices");
}

#[test]
fn the_order_is_the_one_it_was_given() {
    let (mut store, account, _dir) = store();
    for name in ["First", "Second", "Third"] {
        store
            .create_saved_search(account, name, "is:unread")
            .unwrap();
    }
    let names = |s: &Store| -> Vec<String> {
        s.saved_searches(account)
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect()
    };
    assert_eq!(names(&store), ["First", "Second", "Third"]);

    let second = store.saved_searches(account).unwrap()[1].id;
    store.move_saved_search(second, true).unwrap();
    assert_eq!(names(&store), ["Second", "First", "Third"]);
    store.move_saved_search(second, false).unwrap();
    assert_eq!(names(&store), ["First", "Second", "Third"]);

    // At the ends, moving further is a no-op rather than an error: the menu
    // item is there whether or not there is anywhere to go.
    let first = store.saved_searches(account).unwrap()[0].id;
    store.move_saved_search(first, true).unwrap();
    assert_eq!(names(&store), ["First", "Second", "Third"]);
    let last = store.saved_searches(account).unwrap()[2].id;
    store.move_saved_search(last, false).unwrap();
    assert_eq!(names(&store), ["First", "Second", "Third"]);
}

#[test]
fn one_part_changes_and_the_rest_stays() {
    let (mut store, account, _dir) = store();
    let id = store
        .create_saved_search(account, "Waiting", "tag:waiting")
        .unwrap();

    store
        .update_saved_search(id, Some("Waiting on"), None)
        .unwrap();
    let after = store.saved_search(id).unwrap().unwrap();
    assert_eq!(after.name, "Waiting on");
    assert_eq!(after.query, "tag:waiting", "renaming left the query alone");

    store
        .update_saved_search(id, None, Some("tag:waiting -is:unread"))
        .unwrap();
    let after = store.saved_search(id).unwrap().unwrap();
    assert_eq!(after.query, "tag:waiting -is:unread");
    assert_eq!(after.name, "Waiting on", "updating left the name alone");
}

#[test]
fn deleting_one_leaves_the_others_where_they_were() {
    let (mut store, account, _dir) = store();
    for name in ["First", "Second", "Third"] {
        store
            .create_saved_search(account, name, "is:unread")
            .unwrap();
    }
    let second = store.saved_searches(account).unwrap()[1].id;
    store.delete_saved_search(second).unwrap();

    let left: Vec<String> = store
        .saved_searches(account)
        .unwrap()
        .into_iter()
        .map(|x| x.name)
        .collect();
    assert_eq!(left, ["First", "Third"]);
    assert!(store.saved_search(second).unwrap().is_none());
}

#[test]
fn an_account_takes_its_searches_with_it() {
    let (mut store, account, _dir) = store();
    // A second account: `ensure_test_account` inserts, so twice is two.
    let other = store.ensure_test_account().unwrap();
    store
        .create_saved_search(account, "Mine", "is:unread")
        .unwrap();
    store
        .create_saved_search(other, "Theirs", "is:starred")
        .unwrap();

    // One account's sidebar never shows another's, because a search is scoped
    // to the account on screen.
    assert_eq!(store.saved_searches(account).unwrap().len(), 1);
    assert_eq!(store.saved_searches(other).unwrap()[0].name, "Theirs");

    store.remove_account(other).unwrap();
    assert!(store.saved_searches(other).unwrap().is_empty());
    assert_eq!(store.saved_searches(account).unwrap().len(), 1);
}

/// Dragged into a new order, which is the whole list renumbered rather than one
/// row patched — the rule folders and tags follow.
#[test]
fn the_order_can_be_given_whole() {
    let (mut store, account, _dir) = store();
    let ids: Vec<i64> = ["First", "Second", "Third", "Fourth"]
        .iter()
        .map(|n| store.create_saved_search(account, n, "is:unread").unwrap())
        .collect();
    let names = |s: &Store| -> Vec<String> {
        s.saved_searches(account)
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect()
    };

    // Last to first, as a drag to the top would give it.
    store
        .reorder_saved_searches(&[ids[3], ids[0], ids[1], ids[2]])
        .unwrap();
    assert_eq!(names(&store), ["Fourth", "First", "Second", "Third"]);

    // Repeating it changes nothing, because positions are absolute.
    store
        .reorder_saved_searches(&[ids[3], ids[0], ids[1], ids[2]])
        .unwrap();
    assert_eq!(names(&store), ["Fourth", "First", "Second", "Third"]);

    // No two rows share a position, which is what a swap-based reorder used to
    // risk when a drag was interrupted.
    let positions: Vec<i64> = store
        .saved_searches(account)
        .unwrap()
        .into_iter()
        .map(|x| x.position)
        .collect();
    assert_eq!(positions, [0, 1, 2, 3]);

    // An id from nowhere is ignored rather than fatal: a stale window can send
    // one for a search another window has just deleted.
    store.reorder_saved_searches(&[999_999]).unwrap();
    assert_eq!(names(&store), ["Fourth", "First", "Second", "Third"]);
}

/* A query names things rather than pointing at them, so renaming one of them
has to reach the queries. These check the rewriting itself; the parser's own
tests check that a rewritten query reads back as what it means. */

#[test]
fn renaming_a_tag_reaches_the_queries_that_name_it() {
    let (mut store, account, _dir) = store();
    let plain = store
        .create_saved_search(account, "Waiting", "tag:urgent -is:unread")
        .unwrap();
    let spaced = store
        .create_saved_search(account, "Both", "tag:urgent OR tag:later")
        .unwrap();
    let elsewhere = store
        .create_saved_search(account, "Other", "from:sam invoice")
        .unwrap();

    let changed = store
        .rename_in_saved_searches(
            account,
            &Renamed::Tag {
                from: "urgent",
                to: "Needs a reply",
            },
        )
        .unwrap();
    assert_eq!(changed, 2, "the two that name it, and not the third");

    // Quoted on the way out, because the new name has a space in it and a bare
    // one would read back as three terms.
    assert_eq!(
        store.saved_search(plain).unwrap().unwrap().query,
        "tag:\"Needs a reply\" -is:unread"
    );
    assert_eq!(
        store.saved_search(spaced).unwrap().unwrap().query,
        "tag:\"Needs a reply\" OR tag:later"
    );
    assert_eq!(
        store.saved_search(elsewhere).unwrap().unwrap().query,
        "from:sam invoice",
        "left exactly as it was typed"
    );
}

#[test]
fn another_accounts_searches_are_not_touched() {
    let (mut store, mine, _dir) = store();
    let theirs = store.ensure_test_account().unwrap();
    let ours = store
        .create_saved_search(mine, "Mine", "tag:urgent")
        .unwrap();
    let them = store
        .create_saved_search(theirs, "Theirs", "tag:urgent")
        .unwrap();

    store
        .rename_in_saved_searches(
            mine,
            &Renamed::Tag {
                from: "urgent",
                to: "pressing",
            },
        )
        .unwrap();

    assert_eq!(
        store.saved_search(ours).unwrap().unwrap().query,
        "tag:pressing"
    );
    assert_eq!(
        store.saved_search(them).unwrap().unwrap().query,
        "tag:urgent",
        "their tag is their own"
    );
}

#[test]
fn renaming_a_folder_reaches_every_way_a_query_can_name_it() {
    let (mut store, account, _dir) = store();
    let leaf = store
        .create_saved_search(account, "Leaf", "in:receipts annex")
        .unwrap();
    let whole = store
        .create_saved_search(account, "Path", "in:\"clients/receipts\"")
        .unwrap();
    let child = store
        .create_saved_search(account, "Child", "in:\"clients/receipts/2026\"")
        .unwrap();
    let other = store
        .create_saved_search(account, "Other", "in:inbox")
        .unwrap();

    store
        .rename_in_saved_searches(
            account,
            &Renamed::Folder {
                from: "Clients/Receipts",
                to: "Clients/Invoices",
            },
        )
        .unwrap();

    let q = |id| store.saved_search(id).unwrap().unwrap().query;
    assert_eq!(q(leaf), "in:invoices annex", "named by its last part");
    assert_eq!(q(whole), "in:clients/invoices", "named by its whole path");
    assert_eq!(
        q(child),
        "in:clients/invoices/2026",
        "carried with its parent"
    );
    assert_eq!(q(other), "in:inbox", "another mailbox entirely");
}
