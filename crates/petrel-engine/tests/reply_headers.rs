//! What a reply needs to thread at the other end: the message's own
//! Message-ID and the ids it referenced, bare, so the composer can write
//! In-Reply-To and References that every other client will follow.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn mail(msgid: Option<&str>, subject: &str, refs: &[&str]) -> Vec<u8> {
    let mut headers = format!(
        "From: Someone <someone@example.com>\r\nTo: me@example.com\r\n\
         Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n"
    );
    if let Some(id) = msgid {
        headers.push_str(&format!("Message-ID: <{id}>\r\n"));
    }
    if !refs.is_empty() {
        let list: Vec<String> = refs.iter().map(|r| format!("<{r}>")).collect();
        headers.push_str(&format!("References: {}\r\n", list.join(" ")));
    }
    format!("{headers}\r\nbody\r\n").into_bytes()
}

#[test]
fn a_thread_message_carries_its_own_id_and_its_references_bare() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let inbox = store.ensure_folder(a, "inbox", "INBOX").unwrap();
    let root = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(1),
            &mail(Some("root@x"), "Plans", &[]),
        )
        .unwrap();
    let reply = store
        .ingest_raw(
            &blobs,
            a,
            Some(inbox),
            Some(2),
            &mail(Some("reply@x"), "Re: Plans", &["root@x", "other@x"]),
        )
        .unwrap();
    let thread = store.thread_of(reply.message_id).unwrap().unwrap();

    let detail = store.thread_detail(thread).unwrap();
    let by_id = |id: i64| detail.iter().find(|m| m.id == id).unwrap();
    assert_eq!(by_id(root.message_id).msgid.as_deref(), Some("root@x"));
    assert!(by_id(root.message_id).references.is_empty());
    assert_eq!(by_id(reply.message_id).msgid.as_deref(), Some("reply@x"));
    let mut refs = by_id(reply.message_id).references.clone();
    refs.sort();
    assert_eq!(refs, vec!["other@x".to_string(), "root@x".to_string()]);
    assert!(
        detail
            .iter()
            .all(|m| !m.msgid.as_deref().unwrap_or("").contains('<')),
        "bare ids, never bracketed"
    );

    // The one-message hydrate agrees with the page.
    let one = store.thread_message(reply.message_id).unwrap().unwrap();
    assert_eq!(one.msgid.as_deref(), Some("reply@x"));
    assert_eq!(one.references.len(), 2);
}

#[test]
fn a_stand_in_key_is_not_offered_as_a_message_id() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let a = store.ensure_test_account().unwrap();
    store.set_active_account(a).unwrap();
    let drafts = store.ensure_folder(a, "drafts", "Drafts").unwrap();
    // No Message-ID at all: the store keys it by its bytes.
    let nameless = store
        .ingest_raw(
            &blobs,
            a,
            Some(drafts),
            Some(1),
            &mail(None, "Nameless", &[]),
        )
        .unwrap();
    assert_eq!(
        store
            .thread_message(nameless.message_id)
            .unwrap()
            .unwrap()
            .msgid,
        None
    );
    // A second server copy carries the real id, without the copy suffix.
    let raw = mail(Some("twice@x"), "Twice", &[]);
    store
        .ingest_raw(&blobs, a, Some(drafts), Some(2), &raw)
        .unwrap();
    let copy = store
        .ingest_raw_second_copy(&blobs, a, Some(drafts), 3, &raw)
        .unwrap();
    assert_eq!(
        store
            .thread_message(copy.message_id)
            .unwrap()
            .unwrap()
            .msgid
            .as_deref(),
        Some("twice@x")
    );
}
