//! Spike S1 — store & search core. These tests are the executable form of the
//! storage/search design claims: external-content FTS stays consistent through
//! insert/update/delete, is rebuildable, snippets work, CJK behavior is
//! documented, and (in the ignored benchmark) latency/size numbers are real.

use petrel_engine::store::{MARK_END, MARK_START, NewMessage, Store};
use petrel_testkit::MailboxGen;

fn to_new(account_id: i64, g: petrel_testkit::GenMessage) -> NewMessage {
    NewMessage {
        account_id,
        date_ms: g.date_ms,
        from_addr: g.from_addr,
        from_display: g.from_display,
        to_addr: g.to_addr,
        subject: g.subject,
        body_text: g.body,
    }
}

fn seeded_store(n: usize, seed: u64) -> (Store, i64, Vec<i64>) {
    let mut store = Store::open_in_memory().expect("open");
    let account = store.ensure_test_account().expect("account");
    let msgs: Vec<NewMessage> = MailboxGen::new(seed, n)
        .map(|g| to_new(account, g))
        .collect();
    let ids = store.insert_messages(&msgs).expect("insert");
    (store, account, ids)
}

#[test]
fn external_content_delete_keeps_index_consistent() {
    let (mut store, account, _) = seeded_store(50, 7);
    let special = NewMessage {
        account_id: account,
        date_ms: 1,
        from_addr: "special@example.com".into(),
        from_display: "Special Sender".into(),
        to_addr: "me@example.com".into(),
        subject: "the uniquetoken99 memo".into(),
        body_text: "body mentioning uniquetoken99 exactly once".into(),
    };
    let id = store
        .insert_messages(std::slice::from_ref(&special))
        .unwrap()[0];

    assert_eq!(store.search("uniquetoken99", 10).unwrap().len(), 1);
    store.delete_message(id).unwrap();
    assert_eq!(store.search("uniquetoken99", 10).unwrap().len(), 0);
    store
        .fts_integrity_check()
        .expect("index consistent after delete");
    assert_eq!(store.message_count().unwrap(), 50);
}

#[test]
fn external_content_update_reindexes() {
    let (mut store, _, ids) = seeded_store(10, 11);
    let id = ids[0];
    store
        .update_body(id, "completely fresh corpus with the word almandine")
        .unwrap();
    let hits = store.search("almandine", 10).unwrap();
    assert_eq!(hits.iter().filter(|h| h.message_id == id).count(), 1);
    store
        .fts_integrity_check()
        .expect("index consistent after update");
}

#[test]
fn rebuild_reproduces_identical_results() {
    let (mut store, _, ids) = seeded_store(200, 13);
    for id in ids.iter().step_by(7).take(20) {
        store.delete_message(*id).unwrap();
    }
    let queries = ["meeting", "quarterly report", "falcon", "invoice", "proj"];
    let before: Vec<Vec<i64>> = queries
        .iter()
        .map(|q| {
            store
                .search(q, 50)
                .unwrap()
                .iter()
                .map(|h| h.message_id)
                .collect()
        })
        .collect();
    store.rebuild_fts().expect("rebuild");
    store
        .fts_integrity_check()
        .expect("consistent after rebuild");
    let after: Vec<Vec<i64>> = queries
        .iter()
        .map(|q| {
            store
                .search(q, 50)
                .unwrap()
                .iter()
                .map(|h| h.message_id)
                .collect()
        })
        .collect();
    assert_eq!(
        before, after,
        "rebuild must reproduce identical result sets"
    );
}

#[test]
fn snippet_highlights_match() {
    let (mut store, account, _) = seeded_store(5, 17);
    let m = NewMessage {
        account_id: account,
        date_ms: 2,
        from_addr: "a@example.com".into(),
        from_display: "A".into(),
        to_addr: "b@example.com".into(),
        subject: "note".into(),
        body_text: "the migration plan mentions heliotrope pigments in section four".into(),
    };
    store.insert_messages(std::slice::from_ref(&m)).unwrap();
    let hits = store.search("heliotrope", 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0]
            .snippet
            .contains(&format!("{MARK_START}heliotrope{MARK_END}")),
        "snippet: {:?}",
        hits[0].snippet
    );
}

#[test]
fn as_you_type_prefix_matches() {
    let (mut store, account, _) = seeded_store(30, 19);
    let m = NewMessage {
        account_id: account,
        date_ms: 3,
        from_addr: "pm@example.com".into(),
        from_display: "PM".into(),
        to_addr: "me@example.com".into(),
        subject: "Xylograph kickoff".into(),
        body_text: "planning the xylograph effort".into(),
    };
    let id = store.insert_messages(std::slice::from_ref(&m)).unwrap()[0];
    for q in ["xy", "xyl", "xylo", "xylogr"] {
        let hits = store.search(q, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.message_id == id),
            "prefix query {q:?} should reach the message"
        );
    }
}

/// Q21: short CJK queries must return results. The built-in trigram tokenizer
/// could not do this — it matches nothing shorter than 3 characters — so the CJK
/// path indexes one token per character instead.
#[test]
fn cjk_short_queries_match() {
    let (mut store, account, _) = seeded_store(5, 23);
    let m = NewMessage {
        account_id: account,
        date_ms: 4,
        from_addr: "jp@example.com".into(),
        from_display: "JP".into(),
        to_addr: "me@example.com".into(),
        subject: "計画".into(),
        body_text: "東京計画の詳細を確認してください".into(),
    };
    let id = store.insert_messages(std::slice::from_ref(&m)).unwrap()[0];

    for q in ["東", "東京", "東京計", "計画"] {
        let hits = store.search(q, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.message_id == id),
            "CJK query {q:?} ({} chars) must match",
            q.chars().count()
        );
    }
}

#[test]
fn hostile_queries_never_error() {
    let (store, _, _) = seeded_store(20, 29);
    for q in [
        "\"unterminated",
        "a\"b OR c",
        "NEAR( AND NOT",
        "col:val*",
        "();DROP TABLE messages;--",
        "*",
        "\u{0000}odd",
    ] {
        store
            .search(q, 10)
            .unwrap_or_else(|e| panic!("query {q:?} errored: {e}"));
    }
}

/// The numbers test. Run explicitly:
/// `cargo test --release -p petrel-engine --test store_spike -- --ignored --nocapture`
/// Message count via PETREL_BENCH_N (default 100_000).
#[test]
#[ignore]
fn bench_insert_and_search() {
    use std::time::Instant;

    let n: usize = std::env::var("PETREL_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench.db");
    let mut store = Store::open(&db_path).unwrap();
    let account = store.ensure_test_account().unwrap();

    let t0 = Instant::now();
    let mut inserted = 0usize;
    let mut body_bytes = 0usize;
    let mut generator = MailboxGen::new(42, n);
    loop {
        let batch: Vec<NewMessage> = generator
            .by_ref()
            .take(1000)
            .map(|g| to_new(account, g))
            .collect();
        if batch.is_empty() {
            break;
        }
        body_bytes += batch
            .iter()
            .map(|m| m.body_text.len() + m.subject.len())
            .sum::<usize>();
        inserted += batch.len();
        store.insert_messages(&batch).unwrap();
    }
    let insert_secs = t0.elapsed().as_secs_f64();

    let t1 = Instant::now();
    store.optimize_fts().unwrap();
    let optimize_secs = t1.elapsed().as_secs_f64();

    let db_bytes = store.db_size_bytes().unwrap();
    println!("--- S1 bench: {inserted} messages ---");
    println!(
        "insert+index: {insert_secs:.1}s  ({:.0} msg/s) · optimize: {optimize_secs:.1}s",
        inserted as f64 / insert_secs
    );
    println!(
        "text volume: {:.1} MB · db size: {:.1} MB ({:.2}x text)",
        body_bytes as f64 / 1e6,
        db_bytes as f64 / 1e6,
        db_bytes as f64 / body_bytes as f64
    );

    let lat = |label: &str, q: &str| {
        // warm
        for _ in 0..3 {
            store.search(q, 20).unwrap();
        }
        let mut times: Vec<f64> = (0..100)
            .map(|_| {
                let t = Instant::now();
                let hits = store.search(q, 20).unwrap();
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(hits);
                ms
            })
            .collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = times[49];
        let p95 = times[94];
        let count = store.search(q, 20).unwrap().len();
        println!(
            "query {label:<22} {q:<22} p50 {p50:7.2}ms  p95 {p95:7.2}ms  (top-20 of {count} shown)"
        );
        p95
    };

    let budget = 200.0;
    let mut worst: f64 = 0.0;
    worst = worst.max(lat("common-term", "meeting"));
    worst = worst.max(lat("two-terms", "quarterly report"));
    worst = worst.max(lat("phrase", "\"status update\""));
    worst = worst.max(lat("rare-token", &MailboxGen::rare_token(5000)));
    worst = worst.max(lat("as-you-type-2", "pr"));
    worst = worst.max(lat("as-you-type-4", "proj"));
    worst = worst.max(lat("cjk-3char", "東京計"));

    println!("worst p95: {worst:.2}ms vs {budget:.0}ms budget");
    assert!(
        worst < budget,
        "search budget exceeded: {worst:.2}ms >= {budget}ms"
    );
}

/// Opening a mailbox at scale — the other half of the exit bar, which the
/// search bench above never measured.
///
/// The list is a different query from search and has its own history: at six
/// thousand messages it once took ten seconds, because grouping by
/// `coalesce(thread_id, -id)` is an expression no plain column index can
/// serve (migration 0015 added one on the expression itself). Search being
/// fast says nothing about it, so it is timed here on the same store.
///
/// Messages are placed in an inbox folder rather than merely inserted:
/// membership *is* the inbox predicate, so a store of unplaced rows would
/// time a query that returns nothing.
///
/// `cargo test --release -p petrel-engine --test store_spike -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_list_open() {
    use petrel_engine::store::ListView;
    use std::time::Instant;

    let n: usize = std::env::var("PETREL_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("bench.db")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();

    let mut generator = MailboxGen::new(42, n);
    let mut uid = 1u32;
    loop {
        let batch: Vec<NewMessage> = generator
            .by_ref()
            .take(1000)
            .map(|g| to_new(account, g))
            .collect();
        if batch.is_empty() {
            break;
        }
        let ids = store.insert_messages(&batch).unwrap();
        for id in ids {
            store.place_message_at(id, inbox, uid).unwrap();
            uid += 1;
        }
    }
    let placed = store.conversations_in(&ListView::parse("inbox")).unwrap();
    println!("--- list-open bench: {n} messages, {placed} conversations ---");

    let time = |label: &str, offset: u32| {
        let view = ListView::parse("inbox");
        for _ in 0..3 {
            store
                .list_threads(&view, offset, 50, petrel_engine::store::Sort::default())
                .unwrap();
        }
        let mut times: Vec<f64> = (0..50)
            .map(|_| {
                let t = Instant::now();
                let rows = store
                    .list_threads(&view, offset, 50, petrel_engine::store::Sort::default())
                    .unwrap();
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(rows);
                ms
            })
            .collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (p50, p95) = (times[24], times[47]);
        println!("list {label:<16} offset {offset:<7} p50 {p50:7.2}ms  p95 {p95:7.2}ms");
        p50
    };

    // Opening the mailbox, and scrolling deep into it — the page a person
    // reaches after a minute of flicking, where OFFSET has the most to skip.
    let open = time("open", 0);
    let deep = time("scrolled", (n as u32 / 2).min(50_000));

    let t = Instant::now();
    let counted = store.conversations_in(&ListView::parse("inbox")).unwrap();
    let count_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("count_view: {count_ms:.2}ms ({counted} conversations)");

    // The exit bar: a cached list opens in under 150ms.
    assert!(
        open < 150.0,
        "list open budget exceeded: {open:.2}ms >= 150ms"
    );
    assert!(
        deep < 150.0,
        "deep-scroll list budget exceeded: {deep:.2}ms >= 150ms"
    );
}

/// The grammar at scale — `search_threads`, which is what the app calls, and
/// which the first bench above never reaches: it times the message-level
/// lookup underneath.
///
/// It was written to ask whether exclusion, `OR`, dates and the operators
/// that join other tables fit the same 200ms. The first run found that the
/// path they were being added to did not: `meeting` alone took 1.6s at a
/// hundred thousand messages, and `after:2020 meeting` — what the "This year"
/// chip writes beside a word — took 1.6s at five thousand. Both are fixed in
/// `store/search.rs`, where the comments say how, and both are why this bench
/// asks two kinds of question.
///
/// **The budget, on the median.** "meeting" is in half of this mailbox, which
/// no word is in a real one; it is the stress case, and ranking it is most of
/// what these queries cost. The median is what is held to 200ms. P95 is
/// printed and not asserted: over fifty runs it is the third-worst sample,
/// and on a laptop ten minutes into a benchmark it moves by forty percent
/// with the temperature while the median stays where it is.
///
/// **What a condition may cost, relative to the words alone.** This is the
/// guard for the bug that was actually here. A condition that can use an
/// index tempts the planner to turn the join round and probe the full-text
/// index once per message, and that is not ten percent slower, it is three
/// hundred times slower, at any mailbox size. So every query built on
/// "meeting" has to cost about what "meeting" costs, which holds on a slow
/// runner at ten thousand messages exactly as it does here.
///
/// `PETREL_BENCH_BASELINE=1` runs only the queries that meant the same thing
/// to the first parser, so this file can be run against an older tree, and
/// `PETREL_BENCH_ONLY=<label>,<label>` times only those rows.
///
/// `cargo test --release -p petrel-engine --test store_spike -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_search_grammar() {
    use std::time::Instant;

    let n: usize = std::env::var("PETREL_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let baseline_only = std::env::var("PETREL_BENCH_BASELINE").is_ok();
    let only = std::env::var("PETREL_BENCH_ONLY").ok();
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("bench.db")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    // Every real account has these, and every search has to look past them.
    store.ensure_folder(account, "trash", "Trash").unwrap();
    store.ensure_folder(account, "spam", "Junk").unwrap();

    let mut generator = MailboxGen::new(42, n);
    let mut uid = 1u32;
    loop {
        let batch: Vec<NewMessage> = generator
            .by_ref()
            .take(1000)
            .map(|g| to_new(account, g))
            .collect();
        if batch.is_empty() {
            break;
        }
        for id in store.insert_messages(&batch).unwrap() {
            store.place_message_at(id, inbox, uid).unwrap();
            uid += 1;
        }
    }
    store.optimize_fts().unwrap();
    println!("--- grammar bench: {n} messages ---");

    let mut medians: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    let mut lat = |label: &'static str, q: &str| {
        if only
            .as_deref()
            .is_some_and(|wanted| !wanted.split(',').any(|w| w == label))
        {
            return;
        }
        for _ in 0..3 {
            store.search_threads(q, 200).unwrap();
        }
        let mut times: Vec<f64> = (0..50)
            .map(|_| {
                let t = Instant::now();
                let rows = store.search_threads(q, 200).unwrap();
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(rows);
                ms
            })
            .collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (p50, p95) = (times[24], times[47]);
        let count = store.search_threads(q, 200).unwrap().len();
        println!("query {label:<20} {q:<44} p50 {p50:7.2}ms  p95 {p95:7.2}ms  ({count} rows)");
        medians.insert(label, p50);
    };

    // What the first parser already understood, unchanged in meaning.
    for (label, q) in [
        ("common-term", "meeting"),
        ("other-term", "budget"),
        ("two-terms", "quarterly report"),
        ("as-you-type-2", "pr"),
        ("words+from", "from:avery meeting"),
        ("words+in", "in:inbox meeting"),
        ("from-alone", "from:avery"),
        ("unread-alone", "is:unread"),
        ("year-alone", "after:2020"),
        // What the "This year" chip writes, beside a word.
        ("year+words", "after:2020 meeting"),
        ("nothing-meets-it", "has:attachment"),
    ] {
        lat(label, q);
    }
    if !baseline_only {
        for (label, q) in [
            ("phrase", "\"status update\""),
            ("exclude", "meeting -report"),
            ("exclude-alone", "-meeting"),
            ("exclude-operator", "meeting -from:avery"),
            ("or-words", "meeting OR budget"),
            ("or-conditions", "from:avery OR from:blake"),
            ("or-mixed", "from:avery OR quarterly report"),
            // Brackets keep like with like, so these are one statement each
            // and have to cost what their words cost.
            ("grouped-senders", "(from:avery OR from:blake) meeting"),
            ("grouped-words", "(meeting OR budget) from:avery"),
            (
                "both-grouped",
                "(from:avery OR from:blake) (meeting OR budget)",
            ),
            ("excluded-group", "meeting -(report OR from:avery)"),
            ("subject", "subject:meeting"),
            ("to", "to:casey meeting"),
            ("to-alone", "to:casey"),
            // The generator starts on 2020-09-13 and adds a minute a message,
            // so these days exist at ten thousand messages as well as at a
            // hundred thousand.
            ("three-days", "after:2020-09-14 before:2020-09-17 meeting"),
            ("one-day-alone", "date:2020-09-15"),
            ("not-ascii", "from:élodie meeting"),
            ("not-ascii-alone", "from:élodie"),
        ] {
            lat(label, q);
        }
    }

    let budget = 200.0;
    let worst = medians.values().copied().fold(0.0, f64::max);
    println!("worst median: {worst:.2}ms vs {budget:.0}ms budget");
    assert!(
        worst < budget,
        "search budget exceeded: {worst:.2}ms >= {budget}ms"
    );

    // Half as much again, and five milliseconds for the noise that is most of
    // a small number. The bug this catches costs hundreds of times over.
    let words = |labels: &[&str]| labels.iter().map(|l| medians.get(l)).sum::<Option<f64>>();
    for (label, built_on) in [
        ("words+from", &["common-term"][..]),
        ("words+in", &["common-term"]),
        ("year+words", &["common-term"]),
        ("exclude", &["common-term"]),
        ("exclude-operator", &["common-term"]),
        ("subject", &["common-term"]),
        ("to", &["common-term"]),
        ("three-days", &["common-term"]),
        ("not-ascii", &["common-term"]),
        ("or-words", &["common-term", "other-term"]),
        ("grouped-senders", &["common-term"]),
        ("grouped-words", &["common-term", "other-term"]),
        ("both-grouped", &["common-term", "other-term"]),
        ("excluded-group", &["common-term"]),
    ] {
        let (Some(cost), Some(alone)) = (medians.get(label), words(built_on)) else {
            continue;
        };
        assert!(
            *cost <= alone * 1.5 + 5.0,
            "{label} costs {cost:.2}ms where its words alone cost {alone:.2}ms"
        );
    }
}
