//! Q21 — short-query CJK search.
//!
//! The CJK path indexes one token per character and issues multi-character
//! queries as phrases, so 1- and 2-character words match while adjacency still
//! constrains the result. Korean is covered by the same mechanism on
//! *precomposed* syllables: 한 is a single codepoint in the text that arrives in
//! mail, so 한국 is two tokens, exactly like 東京.

use petrel_engine::store::{NewMessage, Store};

fn store_with(bodies: &[(&str, &str)]) -> (Store, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = bodies
        .iter()
        .enumerate()
        .map(|(i, (subject, body))| NewMessage {
            account_id: account,
            date_ms: 1000 + i as i64,
            from_addr: "a@example.com".into(),
            from_display: "A".into(),
            to_addr: "me@example.com".into(),
            subject: (*subject).into(),
            body_text: (*body).into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    (store, ids)
}

#[test]
fn one_and_two_character_queries_match_across_scripts() {
    let (store, ids) = store_with(&[
        ("会議", "明日の会議は東京で行います"),          // Japanese
        ("合同", "上海の合同会议について"),               // Simplified Chinese
        ("회의", "내일 서울에서 회의가 있습니다"),        // Korean, precomposed
    ]);

    // one character
    for (q, want) in [("会", 0usize), ("海", 1), ("서", 2)] {
        let hits = store.search(q, 20).unwrap();
        assert!(
            hits.iter().any(|h| h.message_id == ids[want]),
            "1-char query {q:?} must match message {want}"
        );
    }

    // two characters — the case the trigram tokenizer could never serve
    for (q, want) in [("会議", 0usize), ("上海", 1), ("회의", 2)] {
        let hits = store.search(q, 20).unwrap();
        assert!(
            hits.iter().any(|h| h.message_id == ids[want]),
            "2-char query {q:?} must match message {want}"
        );
    }
}

#[test]
fn korean_two_syllable_word_is_two_tokens_not_six_jamo() {
    // 한국 is 2 codepoints precomposed. Petrel deliberately does not decompose to
    // jamo: per-syllable tokens are the unit, so this matches as a 2-token phrase.
    let (store, ids) = store_with(&[("공지", "한국 지사 공지사항입니다")]);
    assert_eq!("한국".chars().count(), 2, "precomposed Hangul, not jamo");

    let hits = store.search("한국", 10).unwrap();
    assert!(hits.iter().any(|h| h.message_id == ids[0]));
}

#[test]
fn multi_character_queries_require_adjacency() {
    // 東 and 京 both appear, but never next to each other: a per-character index
    // without phrase queries would wrongly match this.
    let (store, ids) = store_with(&[
        ("出張", "東の空と京の街を別々に見た"),
        ("旅行", "東京に行きます"),
    ]);

    let hits = store.search("東京", 20).unwrap();
    assert!(
        hits.iter().any(|h| h.message_id == ids[1]),
        "adjacent 東京 must match"
    );
    assert!(
        !hits.iter().any(|h| h.message_id == ids[0]),
        "non-adjacent 東 … 京 must not match"
    );
}

#[test]
fn latin_search_is_unaffected() {
    let (store, ids) = store_with(&[("Quarterly report", "the annex is attached")]);
    for q in ["annex", "quarterly", "ann"] {
        let hits = store.search(q, 10).unwrap();
        assert!(
            hits.iter().any(|h| h.message_id == ids[0]),
            "Latin query {q:?} must still work"
        );
    }
}

#[test]
fn mixed_script_query_requires_both_parts() {
    let (store, ids) = store_with(&[
        ("報告", "東京 quarterly report attached"),
        ("報告", "大阪 quarterly report attached"),
    ]);
    let hits = store.search("東京 quarterly", 20).unwrap();
    assert!(hits.iter().any(|h| h.message_id == ids[0]));
    assert!(
        !hits.iter().any(|h| h.message_id == ids[1]),
        "the CJK half must still constrain the match"
    );
}

#[test]
fn non_cjk_messages_are_not_indexed_in_the_cjk_table() {
    // The CJK index stays empty for mailboxes that have no CJK, which is what
    // keeps it from doubling storage for everyone else.
    let (store, _) = store_with(&[
        ("Quarterly report", "no cjk here at all"),
        ("Invoice", "still nothing"),
    ]);
    assert_eq!(store.cjk_indexed_count().unwrap(), 0);

    let (store2, _) = store_with(&[("会議", "東京"), ("Invoice", "plain latin")]);
    assert_eq!(
        store2.cjk_indexed_count().unwrap(),
        1,
        "only the message containing CJK is indexed"
    );
}

#[test]
fn deleting_a_message_removes_it_from_the_cjk_index() {
    let (mut store, ids) = store_with(&[("会議", "東京の会議")]);
    assert_eq!(store.cjk_indexed_count().unwrap(), 1);
    store.delete_message(ids[0]).unwrap();
    assert_eq!(store.cjk_indexed_count().unwrap(), 0);
    assert!(store.search("東京", 10).unwrap().is_empty());
}

#[test]
fn rebuild_restores_the_cjk_index_from_content() {
    let (store, ids) = store_with(&[("会議", "東京の会議")]);
    store.rebuild_fts().unwrap();
    let hits = store.search("会議", 10).unwrap();
    assert!(hits.iter().any(|h| h.message_id == ids[0]));
    store.fts_integrity_check().unwrap();
}

#[test]
fn snippets_come_from_the_original_text_not_the_spaced_index() {
    let (store, _) = store_with(&[("会議", "明日の会議は東京で行います")]);
    let hits = store.search("東京", 10).unwrap();
    let snip = &hits[0].snippet;
    assert!(snip.contains("[東京]"), "match should be bracketed: {snip}");
    assert!(
        !snip.contains("東 京"),
        "snippet must not show the space-separated index copy: {snip}"
    );
}

#[test]
fn hostile_cjk_queries_never_error() {
    let (store, _) = store_with(&[("会議", "東京の会議")]);
    for q in ["東\"京", "東\0京", "東 \" 京", "\"", "東京\"\"", "  東  ", "東-京"] {
        store
            .search(q, 10)
            .unwrap_or_else(|e| panic!("query {q:?} errored: {e}"));
    }
}

/// What the per-character index actually costs on disk. Measured by dropping it
/// and vacuuming, rather than asserted from theory.
/// `cargo test -p petrel-engine --test cjk_search -- --ignored --nocapture cost`
#[test]
#[ignore = "measurement, not an assertion"]
fn cjk_index_disk_cost() {
    use std::fmt::Write as _;
    let dir = std::env::temp_dir().join(format!("petrel-cjk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cost.db");
    let _ = std::fs::remove_file(&path);

    let n = 4000usize;
    let mut store = Store::open(&path).unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs: Vec<NewMessage> = (0..n)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 1000 + i as i64,
            from_addr: "a@example.com".into(),
            from_display: "A".into(),
            to_addr: "me@example.com".into(),
            subject: format!("会議 {i} の件"),
            body_text: format!(
                "東京支社の会議{i}について、明日の午前中に詳細を確認してください。\
                 資料は添付のとおりです。担当者は営業部の田中さんになります。"
            ),
        })
        .collect();
    store.insert_messages(&msgs).unwrap();
    let indexed = store.cjk_indexed_count().unwrap();
    drop(store);

    let with = std::fs::metadata(&path).unwrap().len();

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("DELETE FROM fts_cjk", []).unwrap();
    conn.execute_batch("VACUUM").unwrap();
    drop(conn);
    let without = std::fs::metadata(&path).unwrap().len();

    let mut out = String::new();
    writeln!(out, "\n  messages indexed : {indexed}").unwrap();
    writeln!(out, "  db with fts_cjk  : {:>9} bytes", with).unwrap();
    writeln!(out, "  db without       : {:>9} bytes", without).unwrap();
    writeln!(
        out,
        "  index cost       : {:>9} bytes  ({:.0}% of db, {:.0} bytes/message)",
        with - without,
        100.0 * (with - without) as f64 / with as f64,
        (with - without) as f64 / indexed as f64
    )
    .unwrap();
    println!("{out}");
    let _ = std::fs::remove_dir_all(&dir);
}
