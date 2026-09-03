//! A search answers with this account's mail, however much of the word the
//! other account holds.
//!
//! The ranking is global and the account filter is applied to its hits, so a
//! single fixed slice of the ranking could be filled entirely by mail next
//! door — six hundred short matches over there hid the one match here, and
//! the search box said there was nothing at all.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn mail(msgid: &str, subject: &str, date_ms: i64, body: &str) -> Vec<u8> {
    format!(
        "From: Someone <someone@example.com>\r\nTo: me@example.com\r\n\
         Subject: {subject}\r\nMessage-ID: <{msgid}>\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nX-Seq: {date_ms}\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

#[test]
fn another_accounts_mail_cannot_crowd_out_a_hit() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let a = store.ensure_test_account().unwrap();
    let b = store.ensure_test_account().unwrap();
    let ia = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let ib = store.ensure_folder(b, "inbox", "INBOX").unwrap();

    // Account A: one genuine hit, ranked low — a long body dilutes bm25.
    let long = "filler words ".repeat(400) + " quarterly";
    store
        .ingest_raw(
            &blobs,
            a,
            Some(ia),
            Some(1),
            &mail("qa@x", "Notes", 0, &long),
        )
        .unwrap();
    // Account B: 650 short matches, every one of them ranked above it.
    for i in 0..650u32 {
        store
            .ingest_raw(
                &blobs,
                b,
                Some(ib),
                Some(i + 1),
                &mail(
                    &format!("qb{i}@x"),
                    &format!("quarterly {i}"),
                    i as i64,
                    "quarterly",
                ),
            )
            .unwrap();
    }

    store.set_active_account(a).unwrap();
    let hits = store.search_threads("quarterly", 200).unwrap();
    assert_eq!(hits.len(), 1, "the account's own match is found: {hits:?}");
    assert!(
        hits[0].match_snippet.is_some(),
        "and it says why it matched"
    );

    // The other account still sees its own, and nothing of A's.
    store.set_active_account(b).unwrap();
    let theirs = store.search_threads("quarterly", 200).unwrap();
    assert_eq!(theirs.len(), 200, "a full page of its own");
    assert!(theirs.iter().all(|r| r.subject.starts_with("quarterly")));
}

#[test]
fn a_binned_conversation_does_not_crowd_out_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let trash = store.ensure_folder(a, "trash", "Trash").unwrap();

    let long = "filler words ".repeat(400) + " kestrel";
    store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("live@x", "Notes", 0, &long),
        )
        .unwrap();
    for i in 0..120u32 {
        store
            .ingest_raw(
                &blobs,
                a,
                Some(trash),
                Some(1000 + i),
                &mail(&format!("bin{i}@x"), "kestrel", i as i64, "kestrel"),
            )
            .unwrap();
    }

    let hits = store.search_threads("kestrel", 20).unwrap();
    assert_eq!(
        hits.len(),
        1,
        "the bin is not searched, and does not hide: {hits:?}"
    );
    // And asking for the bin by name finds them.
    assert!(
        !store
            .search_threads("in:trash kestrel", 20)
            .unwrap()
            .is_empty()
    );
}
