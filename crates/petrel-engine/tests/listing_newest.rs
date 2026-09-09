//! A list row names the conversation's newest message, not only its own.
//!
//! A row's own message is the newest one *in the view*: an inbox conversation
//! you have answered shows the other side's last message, while the reply
//! sits in Sent. The reading pane opens the conversation's newest, so the row
//! carries that too, and the pane can paint the right body before it has
//! asked for the index.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Sort, SortKey, Store};

fn msg(from: &str, id: &str, reply_to: Option<&str>, date: &str, body: &str) -> Vec<u8> {
    let reply = reply_to
        .map(|r| format!("In-Reply-To: <{r}>\r\nReferences: <{r}>\r\n"))
        .unwrap_or_default();
    format!(
        "From: {from}\r\nTo: me@example.com\r\nSubject: newest\r\nDate: {date}\r\n\
         Message-ID: <{id}>\r\n{reply}MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

fn newest_first() -> Sort {
    Sort {
        key: SortKey::Date,
        ascending: false,
    }
}

#[test]
fn an_answered_inbox_conversation_names_the_reply_as_its_newest() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("petrel.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let theirs = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(1),
            &msg(
                "Sam <sam@example.com>",
                "a@example.com",
                None,
                "Sat, 5 Sep 2026 10:00:00 +0000",
                "hello",
            ),
        )
        .unwrap();
    let mine = store
        .ingest_raw(
            &blobs,
            account,
            Some(sent),
            Some(1),
            &msg(
                "Me <me@example.com>",
                "b@example.com",
                Some("a@example.com"),
                "Sat, 5 Sep 2026 11:00:00 +0000",
                "my reply",
            ),
        )
        .unwrap();

    let rows = store
        .list_threads(&ListView::parse("inbox"), 0, 50, newest_first())
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, theirs.message_id, "the row is the inbox message");
    assert_eq!(row.newest.id, mine.message_id, "its newest is the reply");
    assert_eq!(row.newest.from_addr, "me@example.com");
    assert_eq!(row.newest.snippet, "my reply");

    // The card is the index's last row, field for field.
    let index = store.thread_index(row.thread_id).unwrap();
    let last = index.last().unwrap();
    assert_eq!(row.newest.id, last.id);
    assert_eq!(row.newest.date_ms, last.date_ms);
    assert_eq!(row.newest.from_display, last.from_display);
    assert_eq!(row.newest.unread, last.unread);

    // The same conversation from Sent, by id and from a search.
    let sent_rows = store
        .list_threads(&ListView::parse("sent"), 0, 50, newest_first())
        .unwrap();
    assert_eq!(sent_rows[0].id, mine.message_id);
    assert_eq!(sent_rows[0].newest.id, mine.message_id);
    let by_id = store.thread_by_id(row.thread_id).unwrap().unwrap();
    assert_eq!(by_id.newest.id, mine.message_id);
    let hits = store.search_threads_sorted("hello", 200, None).unwrap();
    assert_eq!(hits.len(), 1, "one conversation matched");
    assert_eq!(hits[0].newest.id, mine.message_id);
}

#[test]
fn a_one_message_conversation_is_its_own_newest() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("petrel.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let only = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(1),
            &msg(
                "Sam <sam@example.com>",
                "solo@example.com",
                None,
                "Sat, 5 Sep 2026 10:00:00 +0000",
                "hello",
            ),
        )
        .unwrap();
    let rows = store
        .list_threads(&ListView::parse("inbox"), 0, 50, newest_first())
        .unwrap();
    assert_eq!(rows[0].id, only.message_id);
    assert_eq!(rows[0].newest.id, only.message_id);
    assert!(rows[0].newest.unread, "fresh mail is unread");
    assert_eq!(rows[0].newest.from_display, "Sam");
}
