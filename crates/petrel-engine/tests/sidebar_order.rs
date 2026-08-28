//! The order somebody drags their folders and tags into.
//!
//! The rule worth pinning is not "the order is saved" but what happens to
//! everything nobody dragged. Arranging one folder must not silently reshuffle
//! the rest, and a folder that arrives from the server afterwards has to land
//! somewhere predictable rather than at the top.

use petrel_engine::store::{AccountServers, Store};

fn store() -> (tempfile::TempDir, Store, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("p.db")).expect("store");
    let account = store
        .add_account("imap", "you@example.com", "You", &AccountServers::default())
        .expect("account");
    (dir, store, account)
}

fn folder_paths(store: &Store, account: i64) -> Vec<String> {
    store
        .folders(account)
        .expect("folders")
        .into_iter()
        .filter(|f| f.role.is_empty())
        .map(|f| f.path)
        .collect()
}

#[test]
fn untouched_folders_stay_alphabetical() {
    let (_dir, store, account) = store();
    for path in ["Zebra", "Apple", "Mango"] {
        store.ensure_named_folder(account, path).expect("folder");
    }
    assert_eq!(folder_paths(&store, account), ["Apple", "Mango", "Zebra"]);
}

#[test]
fn dragging_one_does_not_reshuffle_the_others() {
    let (_dir, mut store, account) = store();
    let mut ids = Vec::new();
    for path in ["Apple", "Mango", "Zebra"] {
        ids.push(store.ensure_named_folder(account, path).expect("folder"));
    }

    // Zebra dragged to the front. Only the two that moved are numbered.
    store.reorder_folders(&[ids[2], ids[0]]).expect("reorder");

    // Zebra then Apple, both arranged; Mango was never dragged and follows.
    assert_eq!(folder_paths(&store, account), ["Zebra", "Apple", "Mango"]);
}

#[test]
fn a_folder_that_arrives_later_lands_after_the_arranged_ones() {
    let (_dir, mut store, account) = store();
    let apple = store.ensure_named_folder(account, "Apple").expect("f");
    let zebra = store.ensure_named_folder(account, "Zebra").expect("f");
    store.reorder_folders(&[zebra, apple]).expect("reorder");

    // The server tells us about a new folder. Alphabetically it would come
    // first; it must not jump above the order somebody chose on purpose.
    store.ensure_named_folder(account, "Aardvark").expect("f");
    assert_eq!(
        folder_paths(&store, account),
        ["Zebra", "Apple", "Aardvark"],
        "a new folder should land after the arranged ones, not at the top"
    );
}

#[test]
fn reordering_twice_settles_rather_than_drifting() {
    let (_dir, mut store, account) = store();
    let mut ids = Vec::new();
    for path in ["A", "B", "C"] {
        ids.push(store.ensure_named_folder(account, path).expect("f"));
    }
    store.reorder_folders(&[ids[2], ids[1], ids[0]]).expect("r");
    assert_eq!(folder_paths(&store, account), ["C", "B", "A"]);

    // Applying the same order again changes nothing, and applying a new one
    // replaces it wholesale rather than accumulating.
    store.reorder_folders(&[ids[2], ids[1], ids[0]]).expect("r");
    assert_eq!(folder_paths(&store, account), ["C", "B", "A"]);
    store.reorder_folders(&[ids[0], ids[2], ids[1]]).expect("r");
    assert_eq!(folder_paths(&store, account), ["A", "C", "B"]);
}

#[test]
fn one_accounts_order_leaves_the_other_alone() {
    let (_dir, mut store, first) = store();
    let second = store
        .add_account("imap", "other@example.com", "Other", &AccountServers::default())
        .expect("second account");
    for account in [first, second] {
        for path in ["Apple", "Zebra"] {
            store.ensure_named_folder(account, path).expect("f");
        }
    }
    let zebra_of_first = store
        .folders(first)
        .expect("folders")
        .into_iter()
        .find(|f| f.path == "Zebra")
        .expect("zebra")
        .id;

    store.reorder_folders(&[zebra_of_first]).expect("reorder");

    assert_eq!(folder_paths(&store, first), ["Zebra", "Apple"]);
    assert_eq!(
        folder_paths(&store, second),
        ["Apple", "Zebra"],
        "the other account should not have moved"
    );
}
