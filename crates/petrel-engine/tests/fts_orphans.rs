//! The index row goes wherever the message row goes.
//!
//! One `fts_content` row with no message behind it used to fail every search
//! that matched it, for good: removing an account cascaded its messages
//! away and left the index behind, and so did retiring a draft's second
//! copy and an autosave landing after a discard.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

const T0: i64 = 1_800_000_000_000;

fn mail(msgid: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Someone <someone@example.com>\r\nTo: me@example.com\r\n\
         Subject: {subject}\r\nMessage-ID: <{msgid}>\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

fn orphan_rows(db: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(
        "SELECT count(*) FROM fts_content WHERE message_id NOT IN (SELECT id FROM messages)",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn removing_an_account_takes_its_index_rows_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("p.db");
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    {
        let mut store = Store::open(&db).unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store.ensure_test_account().unwrap();
        let ia = store.ensure_folder(a, "inbox", "INBOX").unwrap();
        let ib = store.ensure_folder(b, "inbox", "INBOX").unwrap();
        store
            .ingest_raw(
                &blobs,
                a,
                Some(ia),
                Some(1),
                &mail("a1@x", "Zebrafish notes", "zebrafish in a"),
            )
            .unwrap();
        for i in 0..5u32 {
            store
                .ingest_raw(
                    &blobs,
                    b,
                    Some(ib),
                    Some(i + 1),
                    &mail(&format!("b{i}@x"), "Zebrafish notes", "zebrafish in b"),
                )
                .unwrap();
        }
        store.set_active_account(a).unwrap();
        assert_eq!(store.search_threads("zebrafish", 50).unwrap().len(), 1);

        store.remove_account(b).unwrap();
        let hits = store.search_threads("zebrafish", 50).unwrap();
        assert_eq!(hits.len(), 1, "the surviving account still searches");
        assert_eq!(store.search("zebrafish", 50).unwrap().len(), 1);
        // Both indexes followed the rows out, not only the content table.
        store.fts_integrity_check().expect("indexes agree");
    }
    assert_eq!(orphan_rows(&db), 0);
}

#[test]
fn an_autosave_after_a_discard_is_refused_rather_than_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("p.db");
    {
        let store = Store::open(&db).unwrap();
        let a = store.ensure_test_account().unwrap();
        store.set_active_account(a).unwrap();
        let id = store
            .save_draft(a, None, "x@example.com", "Quokka plan", "quokka body", "")
            .unwrap();
        store.delete_draft(id).unwrap();
        let again = store.save_draft(a, Some(id), "x@example.com", "Quokka plan", "again", "");
        assert!(
            again.is_err(),
            "the draft is gone; saving into it is refused"
        );
        assert!(store.search_threads("quokka", 50).unwrap().is_empty());
    }
    assert_eq!(orphan_rows(&db), 0);
}

#[test]
fn retiring_a_second_copy_takes_its_index_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("p.db");
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    {
        let mut store = Store::open(&db).unwrap();
        let a = store.ensure_test_account().unwrap();
        store.set_active_account(a).unwrap();
        let drafts = store.ensure_folder(a, "drafts", "Drafts").unwrap();
        let raw = mail("d1@x", "Wombat draft", "wombat text");
        let first = store
            .ingest_raw(&blobs, a, Some(drafts), Some(1), &raw)
            .unwrap();
        let second = store
            .ingest_raw_second_copy(&blobs, a, Some(drafts), 2, &raw)
            .unwrap();
        assert_ne!(first.message_id, second.message_id);
        store.retire_second_copy(second.message_id).unwrap();
        assert_eq!(store.search_threads("wombat", 50).unwrap().len(), 1);
    }
    assert_eq!(orphan_rows(&db), 0);
}

/// Belt under the braces: a stray index row that got in anyway is skipped,
/// not fatal.
#[test]
fn a_stray_index_row_does_not_fail_the_search() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("p.db");
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let mut store = Store::open(&db).unwrap();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail("s1@x", "Stray", "numbat sighting"),
        )
        .unwrap();
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let flags = rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC;
        conn.create_scalar_function("petrel_cjk", 1, flags, |ctx| ctx.get::<Option<String>>(0))
            .unwrap();
        conn.create_scalar_function("petrel_has_cjk", 1, flags, |_ctx| Ok(false))
            .unwrap();
        conn.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             VALUES (999, 'Stray', 'numbat sighting too', '', '')",
            [],
        )
        .unwrap();
    }
    let hits = store.search_threads("numbat", 50).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(store.search_listing("numbat", 50).unwrap().len(), 1);
    let _ = T0;
}
