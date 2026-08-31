//! M1 — ingestion: raw RFC822 in, searchable message out.
//!
//! This is the seam where the three M0 pieces meet: bytes from a server, the
//! MIME parser, and the store's transactional index. The properties that matter
//! are not "it parsed" but: **a resync cannot duplicate mail**, **the raw bytes
//! survive verbatim**, and **the index describes what was actually stored**.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn fixture(message_id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{message_id}>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         {body}\r\n"
    )
    .into_bytes()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    (dir, store, blobs, account)
}

#[test]
fn ingested_mail_is_searchable_and_byte_exact() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = fixture(
        "vendor-1@example.com",
        "Q3 vendor contracts",
        "Let's lock pricing before Friday. The heliotrope quote is attached in spirit.",
    );

    let out = store
        .ingest_raw(&blobs, account, None, Some(7), &raw)
        .expect("ingest");
    assert!(out.was_new);

    // Searchable by body, subject, and sender — through the real index.
    for query in ["heliotrope", "vendor contracts", "dana@example.com"] {
        let hits = store.search(query, 10).expect("search");
        assert!(
            hits.iter().any(|h| h.message_id == out.message_id),
            "query {query:?} should find the ingested message"
        );
    }

    // The original bytes come back untouched: the parse is a view, not the truth.
    let stored = blobs.read(&out.blob_hash).expect("read blob");
    assert_eq!(stored, raw, "raw message must round-trip byte-for-byte");

    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn resync_of_the_same_message_updates_instead_of_duplicating() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = fixture("stable-id@example.com", "Original subject", "first body");

    let first = store
        .ingest_raw(&blobs, account, None, Some(1), &raw)
        .expect("ingest");
    assert!(first.was_new);

    // A resync re-fetches the identical bytes — the common case, every sync.
    let again = store
        .ingest_raw(&blobs, account, None, Some(1), &raw)
        .expect("re-ingest");
    assert!(
        !again.was_new,
        "same Message-ID must not create a second row"
    );
    assert_eq!(again.message_id, first.message_id);
    assert_eq!(store.message_count().expect("count"), 1);

    // And a server-side edit (same ID, changed content) refreshes the index
    // rather than leaving a stale copy searchable.
    let edited = fixture(
        "stable-id@example.com",
        "Amended subject",
        "second body zephyr",
    );
    let third = store
        .ingest_raw(&blobs, account, None, Some(1), &edited)
        .expect("edit");
    assert_eq!(third.message_id, first.message_id);
    assert_eq!(store.message_count().expect("count"), 1);
    assert_eq!(store.search("zephyr", 10).expect("search").len(), 1);
    assert!(
        store.search("first body", 10).expect("search").is_empty(),
        "superseded content must not linger in the index"
    );
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn message_without_message_id_still_deduplicates() {
    let (_dir, mut store, blobs, account) = setup();
    // Message-ID is a SHOULD, not a MUST; plenty of real mail omits it.
    let raw = b"From: nobody@example.com\r\nSubject: no id here\r\n\r\nbody text\r\n";

    let a = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("first");
    let b = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("second");
    assert_eq!(
        a.message_id, b.message_id,
        "content hash must stand in for a missing Message-ID"
    );
    assert_eq!(store.message_count().expect("count"), 1);
}

#[test]
fn html_only_mail_is_findable_by_its_visible_text() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = b"From: news@example.com\r\n\
Subject: newsletter\r\n\
Message-ID: <html-1@example.com>\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n\
<html><body><p>The <b>quarterly</b> figures are ready.</p>\
<script>var tracking='beacon';</script></body></html>\r\n";

    let out = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("ingest");
    let hits = store.search("quarterly figures", 10).expect("search");
    assert!(hits.iter().any(|h| h.message_id == out.message_id));
    // Script contents must not be searchable — they are not text the user saw.
    assert!(store.search("beacon", 10).expect("search").is_empty());
}

#[test]
fn attachments_are_recorded_and_searchable_by_filename() {
    let (_dir, mut store, blobs, account) = setup();
    let raw = b"From: a@example.com\r\n\
Subject: contract\r\n\
Message-ID: <att-1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=B\r\n\r\n\
--B\r\n\
Content-Type: text/plain\r\n\r\n\
Signed copy attached.\r\n\
--B\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"vendor-agreement.pdf\"\r\n\r\n\
%PDF-1.4\r\n\
--B--\r\n";

    let out = store
        .ingest_raw(&blobs, account, None, None, raw)
        .expect("ingest");
    // Users search for the filename they remember, not the body.
    let hits = store.search("vendor-agreement", 10).expect("search");
    assert!(
        hits.iter().any(|h| h.message_id == out.message_id),
        "attachment filenames belong in the index"
    );
}

#[test]
fn unparseable_input_does_not_poison_the_store() {
    let (_dir, mut store, blobs, account) = setup();
    let good = fixture("ok@example.com", "fine", "readable body");
    store
        .ingest_raw(&blobs, account, None, None, &good)
        .expect("good");

    // Garbage may fail to ingest, but it must leave the store usable and
    // consistent — one bad message can't take the mailbox down with it.
    let _ = store.ingest_raw(&blobs, account, None, None, &[0xFF; 512]);
    let _ = store.ingest_raw(&blobs, account, None, None, b"");

    assert_eq!(store.search("readable", 10).expect("search").len(), 1);
    store.fts_integrity_check().expect("index still consistent");
}

#[test]
fn mail_survives_closing_and_reopening_the_store() {
    // Persistence is the difference between a cache and a mail client. Prove
    // the store reopens with its mail, its index, and its blobs intact —
    // including full-text search, which is derived state and therefore the
    // thing most likely to be silently missing after a restart.
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("petrel.db");
    let blob_dir = dir.path().join("blobs");
    let raw = fixture(
        "persist@example.com",
        "Kept across restarts",
        "cormorant ledger entry",
    );

    let (id, hash) = {
        let mut store = Store::open(&db).expect("store");
        let blobs = BlobStore::open(&blob_dir).expect("blobs");
        let account = store.ensure_test_account().expect("account");
        let out = store
            .ingest_raw(&blobs, account, None, Some(1), &raw)
            .expect("ingest");
        (out.message_id, out.blob_hash)
    }; // both handles dropped: the process is effectively gone

    let store = Store::open(&db).expect("reopen store");
    let blobs = BlobStore::open(&blob_dir).expect("reopen blobs");

    assert_eq!(store.message_count().expect("count"), 1);
    let hits = store.search("cormorant", 10).expect("search");
    assert_eq!(
        hits.len(),
        1,
        "the index must survive a restart, not just the rows"
    );
    assert_eq!(hits[0].message_id, id);
    assert_eq!(
        blobs.read(&hash).expect("blob"),
        raw,
        "bytes survive verbatim"
    );
    assert!(store.first_account().expect("accounts").is_some());
    store
        .fts_integrity_check()
        .expect("index consistent after reopen");
}

/// ISO-2022-JP headers and body, the charset Japanese transactional mail
/// still uses. Synthetic addresses only.
fn iso_2022_jp_mail() -> Vec<u8> {
    let mut raw = b"From: =?ISO-2022-JP?B?GyRCMnE1RCRON28bKEI=?= <info@example.jp>\r\n\
To: me@example.com\r\n\
Subject: =?ISO-2022-JP?B?GyRCMnE1RCRON28bKEI=?=\r\n\
Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
Message-ID: <iso2022jp@example.jp>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=ISO-2022-JP\r\n\
Content-Transfer-Encoding: 7bit\r\n\r\n"
        .to_vec();
    raw.extend_from_slice(b"\x1b$B$3$l$OK\\J8$G$9!#\x1b(B\r\n");
    raw
}

#[test]
fn iso_2022_jp_ingests_readable_and_searchable() {
    let (_dir, mut store, blobs, account) = setup();
    let out = store
        .ingest_raw(&blobs, account, None, Some(1), &iso_2022_jp_mail())
        .expect("ingest");

    let rows = store.list_recent(0, 10).expect("list");
    let row = rows.iter().find(|r| r.id == out.message_id).expect("row");
    assert_eq!(row.subject, "会議の件");
    assert_eq!(row.from_display, "会議の件");
    assert!(
        row.snippet.contains("本文"),
        "snippet was {:?}",
        row.snippet
    );

    let hits = store.search("会議", 10).expect("search");
    assert!(
        hits.iter().any(|h| h.message_id == out.message_id),
        "CJK search must find the decoded subject"
    );
    store.fts_integrity_check().expect("index consistent");
}

#[test]
fn reindex_repairs_legacy_charset_columns() {
    let (_dir, mut store, blobs, account) = setup();
    let out = store
        .ingest_raw(&blobs, account, None, Some(1), &iso_2022_jp_mail())
        .expect("ingest");

    // What the extractor wrote before full_encoding: Base64 unwrapped, JIS
    // left as ESC sequences. List rows and search both saw this.
    let garbled = "\u{1b}$B2q5D$N7o\u{1b}(B";
    let garbled_body = "\u{1b}$B$3$l$OK\\J8$G$9!#\u{1b}(B\r\n";
    store
        .overwrite_extracted(out.message_id, garbled, garbled, garbled_body, garbled_body)
        .expect("plant stale extraction");
    store
        .set_setting("extraction_version", "3")
        .expect("roll version back");

    let stale = store.list_recent(0, 10).expect("list");
    let stale_row = stale.iter().find(|r| r.id == out.message_id).expect("row");
    assert_eq!(stale_row.subject, garbled);
    assert!(
        store.search("会議", 10).expect("search").is_empty(),
        "garbled CJK must not match until reindex"
    );

    let n = store.reindex_bodies(&blobs).expect("reindex");
    assert_eq!(n, 1);
    assert_eq!(store.reindex_bodies(&blobs).expect("second pass"), 0);

    let rows = store.list_recent(0, 10).expect("list");
    let row = rows.iter().find(|r| r.id == out.message_id).expect("row");
    assert_eq!(row.subject, "会議の件");
    assert_eq!(row.from_display, "会議の件");
    assert!(
        row.snippet.contains("本文"),
        "snippet was {:?}",
        row.snippet
    );
    let hits = store.search("会議", 10).expect("search");
    assert!(hits.iter().any(|h| h.message_id == out.message_id));
    store.fts_integrity_check().expect("index consistent");
}

/// The re-extraction in slices, and what survives being interrupted.
///
/// The whole pass is about ninety seconds on a real mailbox, and it runs
/// holding the lock every UI command needs. Slicing it is what keeps the
/// window alive; resuming is what stops an interrupted upgrade starting over
/// from the top every launch.
mod reindex_batches {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::Store;
    use tempfile::TempDir;

    fn fixture(n: usize) -> Vec<u8> {
        format!(
            "From: Dana <dana@example.com>\r\nTo: me@example.com\r\n\
             Subject: message {n}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
             Message-ID: <m{n}@x>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\nbody {n}\r\n"
        )
        .into_bytes()
    }

    fn store_with(count: usize) -> (TempDir, Store, BlobStore) {
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        for n in 0..count {
            store
                .ingest_raw(
                    &blobs,
                    account,
                    Some(inbox),
                    Some(n as u32 + 1),
                    &fixture(n),
                )
                .unwrap();
        }
        // Back to an older extractor, so there is work to do.
        store.set_setting("extraction_version", "0").unwrap();
        (dir, store, blobs)
    }

    #[test]
    fn a_slice_does_its_share_and_says_there_is_more() {
        let (_d, mut store, blobs) = store_with(10);
        let first = store.reindex_batch(&blobs, 4).unwrap();
        assert_eq!(first.done, 4);
        assert!(!first.finished, "four of ten is not finished");
    }

    #[test]
    fn slices_resume_rather_than_repeat() {
        let (_d, mut store, blobs) = store_with(10);
        let mut seen = 0usize;
        let mut slices = 0;
        loop {
            let p = store.reindex_batch(&blobs, 3).unwrap();
            seen += p.done;
            slices += 1;
            if p.finished {
                break;
            }
            assert!(slices < 20, "did not converge — a slice is repeating work");
        }
        // Ten messages in threes: 3, 3, 3, 1, then an empty slice that
        // finishes. Every message exactly once, none twice.
        assert_eq!(seen, 10, "wrong total across slices");
    }

    #[test]
    fn finishing_moves_the_version_and_starting_again_is_free() {
        let (_d, mut store, blobs) = store_with(5);
        while !store.reindex_batch(&blobs, 2).unwrap().finished {}
        // Now current: another pass must do nothing at all.
        let again = store.reindex_batch(&blobs, 2).unwrap();
        assert_eq!(again.done, 0);
        assert!(again.finished);
        assert_eq!(store.reindex_bodies(&blobs).unwrap(), 0);
    }

    #[test]
    fn an_interrupted_pass_is_not_marked_done() {
        let (_d, mut store, blobs) = store_with(10);
        let p = store.reindex_batch(&blobs, 3).unwrap();
        assert!(!p.finished);
        // Quitting here must leave the store still wanting the work, or the
        // seven messages nobody reached keep their old extraction for good.
        let held = store
            .settings()
            .unwrap()
            .get("extraction_version")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        assert!(held < Store::EXTRACTION_VERSION, "marked done at 3 of 10");
        // And picking up where it left off covers the rest.
        assert_eq!(store.reindex_bodies(&blobs).unwrap(), 7);
    }

    #[test]
    fn the_whole_pass_still_works_in_one_call() {
        let (_d, mut store, blobs) = store_with(6);
        assert_eq!(store.reindex_bodies(&blobs).unwrap(), 6);
    }
}

/// Is the explicit FTS rebuild at the end of a re-extraction doing anything?
///
/// It costs 8.5 seconds on a real mailbox — the single longest stretch the
/// store's lock is held, and so the longest the window is frozen. The triggers
/// on `fts_content` already mirror every write into both indexes, so the
/// question is whether the rebuild repairs anything they missed.
#[test]
fn search_finds_re_extracted_text_without_an_explicit_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = petrel_engine::store::Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();

    let raw = b"From: Dana <dana@example.com>\r\nTo: me@example.com\r\n\
Subject: quarterly rutabaga report\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
Message-ID: <r1@x>\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nthe rutabaga numbers are in\r\n";
    let out = store
        .ingest_raw(&blobs, account, Some(inbox), Some(1), raw)
        .unwrap();

    // Garble what was extracted, exactly as an older extractor would have left
    // it, and mark the store as needing the work again.
    store
        .overwrite_extracted(
            out.message_id,
            "mojibake",
            "mojibake",
            "mojibake",
            "mojibake",
        )
        .unwrap();
    store.set_setting("extraction_version", "0").unwrap();
    assert_eq!(
        store.search("rutabaga", 10).unwrap().len(),
        0,
        "the fixture did not actually garble the index"
    );

    // Slices only. Nothing here calls rebuild_fts by hand.
    while !store.reindex_batch(&blobs, 1).unwrap().finished {}

    assert_eq!(
        store.search("rutabaga", 10).unwrap().len(),
        1,
        "re-extracted text never reached the search index"
    );
}
