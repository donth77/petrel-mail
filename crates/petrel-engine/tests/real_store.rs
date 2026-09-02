//! Audits a copy of a real store: every view's count against its listing,
//! and every view walked page by page from the cursor until the oldest
//! conversation. Opt-in, because it needs a store that only you have.
//!
//! Copy `petrel.db` (and its `-wal`) somewhere the app is not writing, then:
//!
//! ```sh
//! PETREL_REAL_DB=/path/to/copy/petrel.db \
//!   cargo test --release -p petrel-engine --test real_store -- --ignored --nocapture
//! ```
//!
//! It prints folder paths and timings to your terminal and nothing else, and
//! it writes nothing but the schema migration a newer build would run anyway.
//! Written for the review of the cursor paging and the placement-based
//! counts, where an eight-message fixture could not say whether the numbers
//! on screen still told the truth at thirty thousand.
use petrel_engine::store::{ListView, Sort, SortKey, Store};
use std::collections::HashSet;
use std::time::{Duration, Instant};

fn open() -> Store {
    let path = std::env::var("PETREL_REAL_DB").expect("set PETREL_REAL_DB to a copy of petrel.db");
    Store::open(std::path::Path::new(&path)).unwrap()
}

/// Every view the rail can show, for one account.
fn views_of(store: &Store, account: i64) -> Vec<(String, ListView)> {
    let mut views: Vec<(String, ListView)> = vec![
        ("inbox".into(), ListView::Inbox),
        ("starred".into(), ListView::Starred),
        ("snoozed".into(), ListView::Snoozed),
        ("outbox".into(), ListView::Outbox),
    ];
    for role in ["archive", "sent", "drafts", "spam", "trash"] {
        views.push((role.into(), ListView::Folder(role.into())));
    }
    for f in store.folders(account).unwrap() {
        if f.role.is_empty() {
            views.push((
                format!("folder#{} {}", f.id, f.path),
                ListView::UserFolder(f.id),
            ));
        }
    }
    views
}

/// The footer's number must be the length of the list it describes.
#[test]
#[ignore]
fn every_count_matches_its_listing() {
    let store = open();
    let mut mismatches = Vec::new();
    for account in store.account_ids().unwrap() {
        store.set_active_account(account).unwrap();
        for (name, view) in views_of(&store, account) {
            let t = Instant::now();
            let n = store.conversations_in(&view).unwrap();
            let dt = t.elapsed();
            let t = Instant::now();
            let rows = store
                .list_threads(&view, 0, 1_000_000, Sort::default())
                .unwrap();
            let dl = t.elapsed();
            let ok = n == rows.len() as i64;
            println!(
                "acct {account} {name}: count={n} ({dt:.1?}) list={} ({dl:.1?}) {}",
                rows.len(),
                if ok { "OK" } else { "MISMATCH" }
            );
            if !ok {
                mismatches.push(format!("acct {account} {name}: {n} vs {}", rows.len()));
            }
        }
        let t = Instant::now();
        let counts = store.view_counts(&Default::default()).unwrap();
        println!(
            "acct {account} rail counts ({:.1?}): {counts:?}",
            t.elapsed()
        );
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Paging from the last row must reach the oldest conversation, in the same
/// order a single listing gives, with nothing repeated and nothing skipped —
/// under every sort, because sender and subject walk a different query.
#[test]
#[ignore]
fn every_view_pages_to_the_end_without_gaps() {
    let store = open();
    let sorts = [
        Sort::default(),
        Sort {
            key: SortKey::Date,
            ascending: true,
        },
        Sort {
            key: SortKey::Sender,
            ascending: false,
        },
        Sort {
            key: SortKey::Subject,
            ascending: true,
        },
    ];
    let mut failures = Vec::new();
    for account in store.account_ids().unwrap() {
        store.set_active_account(account).unwrap();
        for sort in sorts {
            for (name, view) in views_of(&store, account) {
                let all: Vec<i64> = store
                    .list_threads(&view, 0, 1_000_000, sort)
                    .unwrap()
                    .iter()
                    .map(|r| r.thread_id)
                    .collect();
                let mut walked: Vec<i64> = Vec::new();
                let mut pages = 0;
                let mut slowest = Duration::ZERO;
                let mut page = store.list_threads(&view, 0, 100, sort).unwrap();
                loop {
                    pages += 1;
                    walked.extend(page.iter().map(|r| r.thread_id));
                    let Some(last) = page.last() else { break };
                    if page.len() < 100 {
                        break;
                    }
                    let t = Instant::now();
                    page = store
                        .list_threads_after(&view, 100, sort, last.date_ms, last.thread_id)
                        .unwrap();
                    slowest = slowest.max(t.elapsed());
                    assert!(pages < 10_000, "runaway walk in {name}");
                }
                let dups = walked.len() - walked.iter().collect::<HashSet<_>>().len();
                let same = walked == all;
                if all.len() >= 100 || !same {
                    println!(
                        "acct {account} {name} {:?}/{}: {} rows in {pages} pages, slowest page {slowest:.1?}, dups={dups} {}",
                        sort.key,
                        if sort.ascending { "asc" } else { "desc" },
                        all.len(),
                        if same { "SAME-ORDER" } else { "MISMATCH" }
                    );
                }
                if !same {
                    failures.push(format!("acct {account} {name} {:?}", sort.key));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
