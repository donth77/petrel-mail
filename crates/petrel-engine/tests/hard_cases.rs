//! The awkward shapes real mail arrives in, and the awkward moments the
//! store is asked about them.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Sort, SortKey, Store};

const DAY: i64 = 24 * 60 * 60 * 1000;

/// A References header longer than SQLite will take variables for.
///
/// One bind per reference, and a long-running list thread carries tens of
/// thousands: past the limit the ancestor lookup would not run at all, and
/// the message could never be ingested — refetched on every cycle, forever.
#[test]
fn a_message_with_thirty_thousand_references_still_ingests() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    store.set_active_account(account).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();

    // The parent, so the tail of the chain is something to thread onto.
    let parent = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(1),
            b"From: a@example.com\r\nTo: me@example.com\r\nSubject: Long chain\r\n\
              Message-ID: <parent@x>\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\r\nroot\r\n",
        )
        .unwrap();

    let mut refs = String::new();
    for i in 0..32_771u32 {
        refs.push_str(&format!("<r{i}@x> "));
    }
    refs.push_str("<parent@x>");
    let raw = format!(
        "From: b@example.com\r\nTo: me@example.com\r\nSubject: Re: Long chain\r\n\
         Message-ID: <huge@x>\r\nDate: Tue, 18 Aug 2026 15:02:00 +0000\r\n\
         References: {refs}\r\n\r\nlast in a very long chain\r\n"
    );
    let ingested = store
        .ingest_raw(&blobs, account, Some(inbox), Some(2), raw.as_bytes())
        .expect("a long chain is still mail");
    assert_eq!(
        store.thread_of(ingested.message_id).unwrap(),
        store.thread_of(parent.message_id).unwrap(),
        "the nearest ancestor is kept, so it still threads"
    );
    assert_eq!(
        store.search_threads("very long chain", 10).unwrap().len(),
        1
    );
}

/// Two workers fetching the same message write the same bytes at once. The
/// temp file they publish from must not be the same file.
#[test]
fn concurrent_writes_of_the_same_bytes_publish_an_intact_blob() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = std::sync::Arc::new(BlobStore::open(dir.path()).unwrap());
    let bytes: Vec<u8> = b"From: a@example.com\r\nSubject: shared\r\n\r\n"
        .iter()
        .copied()
        .chain(std::iter::repeat_n(b'x', 400_000))
        .collect();
    let mut hands = Vec::new();
    for _ in 0..8 {
        let blobs = std::sync::Arc::clone(&blobs);
        let bytes = bytes.clone();
        hands.push(std::thread::spawn(move || blobs.write(&bytes).unwrap()));
    }
    let hashes: Vec<String> = hands
        .into_iter()
        .map(|h| h.join().unwrap().0)
        .collect::<Vec<_>>();
    assert!(hashes.windows(2).all(|w| w[0] == w[1]));
    // Reading verifies the bytes against the name they are filed under.
    assert_eq!(blobs.read(&hashes[0]).unwrap(), bytes);
    assert_eq!(
        blobs.pending_temp_files().unwrap(),
        0,
        "every temp file was renamed away"
    );
}

/// A tombstone the queue still names is not the garbage collector's.
#[test]
fn gc_leaves_a_message_whose_delete_has_not_reached_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    store.set_active_account(account).unwrap();
    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    let m = store
        .ingest_raw(
            &blobs,
            account,
            Some(trash),
            Some(1),
            b"From: a@example.com\r\nTo: me@example.com\r\nSubject: Delete forever\r\n\
              Message-ID: <df@x>\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\r\nbody\r\n",
        )
        .unwrap();
    let thread = store.thread_of(m.message_id).unwrap().unwrap();
    let receipt = store
        .apply_thread_action(
            account,
            thread,
            ActionKind::DeleteForever,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    assert_eq!(store.pending_actions(account).unwrap().len(), 1);

    // Offline for thirty-one days.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let report = store.gc(&blobs, now + 31 * DAY, 30).unwrap();
    assert_eq!(report.messages_purged, 0, "the expunge still has to go out");
    assert_eq!(
        store.pending_actions(account).unwrap().len(),
        1,
        "and the action that carries it is still queued"
    );

    // Once the server has been told, the row is the collector's.
    store
        .mark_message_outcome(receipt.action_id, m.message_id, true)
        .unwrap();
    let report = store.gc(&blobs, now + 31 * DAY, 30).unwrap();
    assert_eq!(report.messages_purged, 1);
}

/// Paging by sender or subject when the row the page ended on has left the
/// view — archived on another device between one page and the next.
#[test]
fn a_vanished_cursor_row_does_not_end_a_sorted_list() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    store.set_active_account(account).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for i in 0..8u32 {
        let raw = format!(
            "From: s{i} <s{i}@example.com>\r\nTo: me@example.com\r\n\
             Subject: Vanish {i}\r\nMessage-ID: <v{i}@x>\r\n\
             Date: Tue, 18 Aug 2026 14:0{i}:00 +0000\r\n\r\nbody\r\n"
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), Some(i + 1), raw.as_bytes())
            .unwrap();
    }

    for key in [SortKey::Sender, SortKey::Subject] {
        for ascending in [true, false] {
            let sort = Sort { key, ascending };
            let all = store.list_threads(&ListView::Inbox, 0, 20, sort).unwrap();
            let first = store.list_threads(&ListView::Inbox, 0, 3, sort).unwrap();
            let cursor = first.last().unwrap().clone();
            let expected: Vec<i64> = all
                .iter()
                .skip(3)
                .map(|r| r.thread_id)
                .filter(|t| *t != cursor.thread_id)
                .collect();

            // The cursor conversation is archived elsewhere, so it is no
            // longer in the inbox at all.
            store
                .apply_thread_action(
                    account,
                    cursor.thread_id,
                    ActionKind::Archive,
                    None,
                    PlacementPolicy::Exclusive,
                )
                .unwrap();
            let rest = store
                .list_threads_after(&ListView::Inbox, 10, sort, cursor.date_ms, cursor.thread_id)
                .unwrap();
            let ids: Vec<i64> = rest.iter().map(|r| r.thread_id).collect();
            assert_eq!(ids, expected, "{key:?} ascending={ascending}");

            // Put it back for the next round.
            let all_now = store.list_threads(&ListView::All, 0, 20, sort).unwrap();
            let _ = all_now;
            store.place_message(cursor.id, inbox).unwrap();
            store
                .remove_placement(cursor.id, account, "archive")
                .unwrap();
        }
    }
}

/// The switcher's number and the number beside the Inbox are the same fact.
#[test]
fn the_account_unread_count_counts_conversations() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    store.set_active_account(account).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for (i, refs) in [
        (0u32, ""),
        (1, "References: <c0@x>\r\n"),
        (2, "References: <c0@x>\r\n"),
    ] {
        let raw = format!(
            "From: a@example.com\r\nTo: me@example.com\r\nSubject: Count me\r\n\
             Message-ID: <c{i}@x>\r\n{refs}Date: Tue, 18 Aug 2026 14:0{i}:00 +0000\r\n\r\nbody\r\n"
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), Some(i + 1), raw.as_bytes())
            .unwrap();
    }
    let rail = store.view_counts(&Default::default()).unwrap();
    let inbox_badge = rail
        .iter()
        .find(|(k, _)| k == "inbox")
        .map(|(_, n)| *n)
        .unwrap();
    assert_eq!(inbox_badge, 1, "one unread conversation");
    assert_eq!(
        store.accounts().unwrap()[0].unread_count,
        inbox_badge,
        "and the switcher says the same"
    );
}

/// An account set up on 465 for a host that only ever offered STARTTLS is
/// moved to 587 on upgrade; one the person pointed somewhere else is left
/// exactly as they set it.
#[test]
fn icloud_and_outlook_accounts_move_to_the_submission_port() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("old.db");
    let servers = |host: &str, port: u16| petrel_engine::store::AccountServers {
        imap_host: "imap.example.com".into(),
        imap_port: 993,
        smtp_host: host.into(),
        smtp_port: port,
        username: "someone".into(),
        provider: String::new(),
    };
    let (icloud, outlook, own, chosen) = {
        let store = Store::open(&db).unwrap();
        (
            store
                .add_account(
                    "imap",
                    "a@icloud.com",
                    "A",
                    &servers("smtp.mail.me.com", 465),
                )
                .unwrap(),
            store
                .add_account(
                    "imap",
                    "b@outlook.com",
                    "B",
                    &servers("smtp-mail.outlook.com", 465),
                )
                .unwrap(),
            // A host that really does answer on 465 keeps it.
            store
                .add_account("imap", "c@gmail.com", "C", &servers("smtp.gmail.com", 465))
                .unwrap(),
            // And a port the person typed is theirs.
            store
                .add_account(
                    "imap",
                    "d@icloud.com",
                    "D",
                    &servers("smtp.mail.me.com", 2525),
                )
                .unwrap(),
        )
    };
    // What an upgrade does: wind the recorded version back one step and open
    // again, so the new migration is the one that runs.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 25).unwrap();
    }
    let store = Store::open(&db).unwrap();
    let port = |id: i64| store.account_servers(id).unwrap().unwrap().smtp_port;
    assert_eq!(port(icloud), 587, "iCloud moved to the submission port");
    assert_eq!(port(outlook), 587, "Outlook moved to the submission port");
    assert_eq!(port(own), 465, "a host that answers on 465 is left alone");
    assert_eq!(port(chosen), 2525, "a port the person chose is left alone");
    // The rest of the account survived being rewritten as JSON.
    let s = store.account_servers(icloud).unwrap().unwrap();
    assert_eq!(s.imap_host, "imap.example.com");
    assert_eq!(s.username, "someone");
}
