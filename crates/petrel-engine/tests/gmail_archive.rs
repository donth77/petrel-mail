//! Archiving where folders are labels means one thing: the Inbox label
//! comes off every message in the conversation — your own reply included,
//! which Gmail files in the inbox beside the message it answers.
//!
//! The bug this pins: the label sweep placed the reply in the inbox with no
//! number, the Sent exemption then kept that placement through an archive,
//! and the conversation stayed listed here and labelled there for good.

use petrel_engine::actions::{ActionKind, PlacementPolicy};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Sort, Store};

fn mail(from: &str, msgid: &str, subject: &str, refs: &[&str], body: &str) -> Vec<u8> {
    let mut headers = format!(
        "From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\n\
         Message-ID: <{msgid}>\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n"
    );
    if !refs.is_empty() {
        let list: Vec<String> = refs.iter().map(|r| format!("<{r}>")).collect();
        headers.push_str(&format!("References: {}\r\n", list.join(" ")));
    }
    format!("{headers}\r\n{body}\r\n").into_bytes()
}

fn label(name: &str) -> String {
    format!("\"\\\\{name}\"")
}

struct Gmail {
    store: Store,
    blobs: BlobStore,
    _dir: tempfile::TempDir,
    account: i64,
    inbox: i64,
    sent: i64,
    all: i64,
}

fn gmail() -> Gmail {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    store.set_active_account(account).unwrap();
    store.set_account_kind(account, "gmail").unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store
        .ensure_folder(account, "sent", "[Gmail]/Sent Mail")
        .unwrap();
    let all = store
        .ensure_folder(account, "archive", "[Gmail]/All Mail")
        .unwrap();
    Gmail {
        store,
        blobs,
        _dir: dir,
        account,
        inbox,
        sent,
        all,
    }
}

/// Their message in the inbox, my reply fetched from Sent and claimed by
/// All Mail, and the label sweep saying the reply carries \Inbox too.
fn replied_conversation(g: &mut Gmail) -> (i64, i64, i64) {
    let theirs = g
        .store
        .ingest_raw(
            &g.blobs,
            g.account,
            Some(g.inbox),
            Some(1),
            &mail(
                "Them <them@example.com>",
                "t@x",
                "Label sweep",
                &[],
                "theirs",
            ),
        )
        .unwrap();
    let mine = g
        .store
        .ingest_raw(
            &g.blobs,
            g.account,
            Some(g.sent),
            Some(3),
            &mail("me@example.com", "m@x", "Re: Label sweep", &["t@x"], "mine"),
        )
        .unwrap();
    g.store.place_message_at(mine.message_id, g.all, 9).unwrap();
    g.store
        .apply_gmail_labels(
            g.account,
            &[("m@x".to_string(), vec![label("Inbox"), label("Sent")])],
        )
        .unwrap();
    assert_eq!(
        g.store.placement_uid(mine.message_id, g.inbox).unwrap(),
        Some(None),
        "the sweep files by Message-ID and learns no INBOX number"
    );
    let thread = g.store.thread_of(theirs.message_id).unwrap().unwrap();
    (theirs.message_id, mine.message_id, thread)
}

#[test]
fn a_conversation_you_replied_to_can_be_archived_out_of_the_inbox() {
    let mut g = gmail();
    let (theirs, mine, thread) = replied_conversation(&mut g);

    let receipt = g
        .store
        .apply_thread_action(
            g.account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();
    assert_eq!(receipt.message_count, 2, "both members lose the label");
    assert!(
        g.store
            .list_threads(&ListView::Inbox, 0, 10, Sort::default())
            .unwrap()
            .is_empty(),
        "the conversation has left the inbox"
    );
    // The reply is still your reply: in Sent, in All Mail, not in the inbox.
    let mut where_mine = g.store.folders_of(mine).unwrap();
    where_mine.sort();
    let mut expected = vec![g.sent, g.all];
    expected.sort();
    assert_eq!(where_mine, expected);

    // The server is asked to take the label off both — and the reply's row
    // is addressed at the inbox, whose number the drain will ask for, never
    // at the Sent copy, which a move would have carried away.
    let pending = g.store.pending_actions(g.account).unwrap();
    assert_eq!(pending.len(), 2, "{pending:?}");
    let for_theirs = pending.iter().find(|p| p.message_id == theirs).unwrap();
    assert_eq!(for_theirs.uid, Some(1));
    assert_eq!(for_theirs.folder_path, "INBOX");
    let for_mine = pending.iter().find(|p| p.message_id == mine).unwrap();
    assert_eq!(for_mine.uid, None);
    assert_eq!(for_mine.folder_path, "INBOX");
    assert_eq!(for_mine.candidate_paths, vec!["INBOX".to_string()]);
    assert!(
        pending.iter().all(|p| p.folder_path != "[Gmail]/Sent Mail"),
        "nothing moves out of Sent"
    );

    // Undo puts the reply back where the sweep had it.
    assert!(g.store.undo_action(receipt.action_id).unwrap());
    assert_eq!(g.store.placement_uid(mine, g.inbox).unwrap(), Some(None));
    assert_eq!(
        g.store
            .list_threads(&ListView::Inbox, 0, 10, Sort::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_member_outside_the_inbox_is_left_alone_by_a_labels_archive() {
    let mut g = gmail();
    let (_theirs, _mine, thread) = replied_conversation(&mut g);
    // A second reply of mine that Gmail did not put in the inbox.
    let solo = g
        .store
        .ingest_raw(
            &g.blobs,
            g.account,
            Some(g.sent),
            Some(4),
            &mail(
                "me@example.com",
                "solo@x",
                "Re: Label sweep",
                &["t@x"],
                "solo",
            ),
        )
        .unwrap();
    g.store
        .place_message_at(solo.message_id, g.all, 10)
        .unwrap();
    assert_eq!(g.store.thread_of(solo.message_id).unwrap(), Some(thread));

    let receipt = g
        .store
        .apply_thread_action(
            g.account,
            thread,
            ActionKind::Archive,
            None,
            PlacementPolicy::Labels,
        )
        .unwrap();
    assert_eq!(
        receipt.message_count, 2,
        "the member with no label to lose is not touched"
    );
    assert!(
        g.store
            .pending_actions(g.account)
            .unwrap()
            .iter()
            .all(|p| p.message_id != solo.message_id)
    );
    assert_eq!(
        g.store.placement_uid(solo.message_id, g.sent).unwrap(),
        Some(Some(4))
    );
}

#[test]
fn the_sweep_keeps_the_inbox_number_it_is_given() {
    let mut g = gmail();
    let m = g
        .store
        .ingest_raw(
            &g.blobs,
            g.account,
            Some(g.sent),
            Some(3),
            &mail("me@example.com", "n@x", "Numbered", &[], "mine"),
        )
        .unwrap();
    g.store
        .apply_gmail_labels_at(
            g.account,
            &[(
                "n@x".to_string(),
                vec![label("Inbox"), label("Sent")],
                Some(77),
            )],
        )
        .unwrap();
    assert_eq!(
        g.store.placement_uid(m.message_id, g.inbox).unwrap(),
        Some(Some(77))
    );
    // A later sweep with no number keeps the one already held.
    g.store
        .apply_gmail_labels(
            g.account,
            &[("n@x".to_string(), vec![label("Inbox"), label("Sent")])],
        )
        .unwrap();
    assert_eq!(
        g.store.placement_uid(m.message_id, g.inbox).unwrap(),
        Some(Some(77))
    );
    // And now the inbox sweep can prune it like any fetched placement.
    let present = std::collections::HashSet::new();
    assert_eq!(
        g.store.remove_placements_absent(g.inbox, &present).unwrap(),
        1
    );
    assert_eq!(g.store.placement_uid(m.message_id, g.inbox).unwrap(), None);
}

#[test]
fn an_inbox_listing_numbers_or_drops_the_placements_the_sweep_made() {
    let mut g = gmail();
    let (theirs, mine, thread) = replied_conversation(&mut g);
    // A third message the sweep filed in the inbox that the phone has since
    // archived: INBOX no longer lists it.
    let stale = g
        .store
        .ingest_raw(
            &g.blobs,
            g.account,
            Some(g.all),
            Some(11),
            &mail("Them <them@example.com>", "s@x", "Stale", &[], "stale"),
        )
        .unwrap();
    g.store
        .apply_gmail_labels(g.account, &[("s@x".to_string(), vec![label("Inbox")])])
        .unwrap();
    // And one just moved into the inbox here, with the move still queued.
    let moved = g
        .store
        .ingest_raw(
            &g.blobs,
            g.account,
            Some(g.all),
            Some(12),
            &mail(
                "Them <them@example.com>",
                "mv@x",
                "Moved here",
                &[],
                "moved",
            ),
        )
        .unwrap();
    let moved_thread = g.store.thread_of(moved.message_id).unwrap().unwrap();
    g.store
        .apply_thread_action(
            g.account,
            moved_thread,
            ActionKind::Move,
            Some(g.inbox),
            PlacementPolicy::Labels,
        )
        .unwrap();
    assert_eq!(
        g.store.placement_uid(moved.message_id, g.inbox).unwrap(),
        Some(None)
    );

    let listing = [
        (1u32, Some("t@x".to_string())),
        (5, Some("m@x".to_string())),
    ];
    let out = g
        .store
        .reconcile_unaddressed_placements(g.inbox, &listing)
        .unwrap();
    assert_eq!(out.rematched, 1, "the reply learns its INBOX number");
    assert_eq!(out.dropped, 1, "the archived one leaves the inbox");
    assert_eq!(g.store.placement_uid(mine, g.inbox).unwrap(), Some(Some(5)));
    assert_eq!(
        g.store.placement_uid(theirs, g.inbox).unwrap(),
        Some(Some(1))
    );
    assert_eq!(
        g.store.placement_uid(stale.message_id, g.inbox).unwrap(),
        None
    );
    assert!(
        !g.store.search_threads("stale", 10).unwrap().is_empty(),
        "leaving a label is not leaving the server, so nothing is tombstoned"
    );
    assert_eq!(
        g.store.placement_uid(moved.message_id, g.inbox).unwrap(),
        Some(None),
        "a placement with work queued is not the listing's to judge"
    );
    let _ = thread;
}
