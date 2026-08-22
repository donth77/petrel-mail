//! Opens a store at a given path and exercises the newest additions against it.
//!
//! Migrations are the one thing a test on a fresh in-memory database cannot
//! honestly cover: a fresh store is created by the schema, not by the
//! migrations, so a broken migration passes every test and fails on the first
//! real mailbox. This is pointed at a *copy* of one.
//!
//! It prints shapes and counts only — never a subject, sender or address from
//! the mailbox it is run against.

use petrel_engine::store::{CountMode, Store};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: mig_check <path to db>");
    let store = Store::open(std::path::Path::new(&path)).expect("open (runs migrations)");
    let account = store
        .first_account()
        .expect("read accounts")
        .expect("a store with no account proves nothing");

    println!("opened and migrated");
    println!(
        "  trusted senders before: {}",
        store.trusted_senders(account).unwrap().len()
    );

    store
        .trust_sender(account, "Someone@Example.invalid", 1)
        .expect("write to the new table");
    let after = store.trusted_senders(account).unwrap();
    println!("  trusted senders after:  {} ({:?})", after.len(), after);
    store
        .untrust_sender(account, "someone@example.invalid")
        .expect("and take it back");
    println!(
        "  after untrust:          {}",
        store.trusted_senders(account).unwrap().len()
    );

    println!(
        "  has_written_to(unknown): {}",
        store
            .has_written_to(account, "nobody@nowhere.invalid")
            .unwrap()
    );
    println!(
        "  view counts:            {:?}",
        store.view_counts(CountMode::Unread).unwrap()
    );
}
