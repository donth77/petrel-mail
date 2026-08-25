//! What a search result has to carry beyond the row itself.
//!
//! A result that shows the same opening line as every other row cannot say what
//! it was answering. The reason a conversation is in the list is exactly the
//! part the reader needs, and the engine was already computing it and throwing
//! it away at the thread rollup.

use petrel_engine::store::{MARK_END, MARK_START, NewMessage, Store};

/// The matched word, wrapped the way the engine wraps it.
fn marked(word: &str) -> String {
    format!("{MARK_START}{word}{MARK_END}")
}

fn seeded() -> (Store, Vec<i64>) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs = [
        (
            "Q3 vendor contracts",
            "Attaching the revised annex and the pricing sheet for review.",
        ),
        (
            "Draft terms for review",
            "I marked up the annex in track changes, section 4 especially.",
        ),
        (
            "Lunch on Thursday",
            "Nothing to do with contracts at all, just lunch.",
        ),
    ]
    .iter()
    .map(|(subject, body)| NewMessage {
        account_id: account,
        date_ms: 1_700_000_000_000,
        from_addr: "sam@example.com".into(),
        from_display: "Sam Ortiz".into(),
        to_addr: "me@example.com".into(),
        subject: (*subject).into(),
        body_text: (*body).into(),
    })
    .collect::<Vec<_>>();
    let ids = store.insert_messages(&msgs).unwrap();
    // The Inbox means membership now; bare rows are in nobody's inbox.
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &ids {
        store.place_message(*id, inbox).unwrap();
    }
    (store, ids)
}

#[test]
fn a_result_says_why_it_matched() {
    let (store, _) = seeded();
    let hits = store.search_threads("annex", 20).unwrap();
    assert_eq!(hits.len(), 2, "both messages mentioning the annex");

    for hit in &hits {
        let why = hit
            .match_snippet
            .as_deref()
            .unwrap_or_else(|| panic!("no match snippet on {:?}", hit.subject));
        assert!(
            why.contains(&marked("annex")),
            "the matched word should be marked in the snippet: {why}"
        );
    }
}

/// An ordinary list is not a search, and must not pretend to be one.
#[test]
fn an_ordinary_listing_carries_no_match_snippet() {
    let (store, _) = seeded();
    let rows = store
        .list_threads(&petrel_engine::store::ListView::Inbox, 0, 20)
        .unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r.match_snippet.is_none()));
}

/// The snippet shown is the best-ranked hit for that conversation, not
/// whichever message happens to be newest.
#[test]
fn the_snippet_comes_from_the_message_that_matched() {
    let (store, _) = seeded();
    let hits = store.search_threads("pricing", 20).unwrap();
    assert_eq!(hits.len(), 1);
    let why = hits[0].match_snippet.as_deref().unwrap();
    assert!(why.contains(&marked("pricing")), "{why:?}");
}

/// Conditions filter what the words rank.
mod operators {
    use petrel_engine::store::{NewMessage, Store};

    fn mailbox() -> (Store, i64) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let msgs = [
            (
                "Sam Ortiz",
                "sam@example.com",
                "Q3 vendor contracts",
                "the annex is attached",
            ),
            (
                "Dana Wu",
                "dana@example.com",
                "Re: Vendor shortlist",
                "the annex from last year",
            ),
            (
                "Sam Ortiz",
                "sam@example.com",
                "Lunch",
                "nothing about the annex here",
            ),
        ]
        .iter()
        .enumerate()
        .map(|(i, (name, addr, subject, body))| NewMessage {
            account_id: account,
            date_ms: 1_700_000_000_000 + i as i64,
            from_addr: (*addr).into(),
            from_display: (*name).into(),
            to_addr: "me@example.com".into(),
            subject: (*subject).into(),
            body_text: (*body).into(),
        })
        .collect::<Vec<_>>();
        let placed = store.insert_messages(&msgs).unwrap();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        for id in &placed {
            store.place_message(*id, inbox).unwrap();
        }
        (store, account)
    }

    fn subjects(store: &Store, q: &str) -> Vec<String> {
        let mut s: Vec<String> = store
            .search_threads(q, 50)
            .unwrap()
            .into_iter()
            .map(|r| r.subject)
            .collect();
        s.sort();
        s
    }

    #[test]
    fn from_narrows_to_a_sender() {
        let (store, _) = mailbox();
        assert_eq!(
            subjects(&store, "annex"),
            ["Lunch", "Q3 vendor contracts", "Re: Vendor shortlist"]
        );
        assert_eq!(
            subjects(&store, "from:dana annex"),
            ["Re: Vendor shortlist"]
        );
    }

    #[test]
    fn from_matches_the_name_as_well_as_the_address() {
        let (store, _) = mailbox();
        assert_eq!(subjects(&store, "from:Ortiz annex").len(), 2);
    }

    /// A condition with no words is a listing, not an empty result.
    #[test]
    fn conditions_alone_return_everything_that_meets_them() {
        let (store, account) = mailbox();
        let starred = store.search_threads("is:starred", 50).unwrap();
        assert!(starred.is_empty(), "nothing is starred yet");

        let ids = store
            .list_threads(&petrel_engine::store::ListView::Inbox, 0, 50)
            .unwrap();
        store
            .set_flags(ids[0].id, petrel_engine::store::flags::FLAGGED, 0)
            .unwrap();
        let _ = account;

        let now = store.search_threads("is:starred", 50).unwrap();
        assert_eq!(now.len(), 1, "the one that was starred");
    }

    /// The condition applies to the ranked results, not instead of them.
    #[test]
    fn words_and_conditions_are_both_honoured() {
        let (store, _) = mailbox();
        assert!(
            subjects(&store, "from:dana lunch").is_empty(),
            "dana wrote no lunch mail"
        );
    }

    #[test]
    fn an_unknown_operator_is_searched_for_rather_than_refused() {
        let (store, _) = mailbox();
        // Nothing contains this, so it finds nothing — but it must not throw or
        // silently return the whole mailbox.
        assert!(subjects(&store, "wat:ever").is_empty());
    }
}

/// Best match and newest are different orders, and the toggle has to prove it.
///
/// If they coincide the control looks broken, so this builds a case where they
/// cannot: the strongest match is the oldest message.
#[test]
fn best_match_and_newest_are_not_the_same_order() {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let msgs = [
        // Oldest, but says "annex" three times — the best match by a distance.
        (1_000, "Annex", "annex annex annex"),
        (2_000, "Middle", "one mention of the annex here"),
        // Newest, and barely relevant.
        (
            3_000,
            "Newest",
            "a long message about many things, including the annex, \
                           and a great deal else besides that dilutes it",
        ),
    ]
    .iter()
    .map(|(date, subject, body)| NewMessage {
        account_id: account,
        date_ms: *date,
        from_addr: "sam@example.com".into(),
        from_display: "Sam".into(),
        to_addr: "me@example.com".into(),
        subject: (*subject).into(),
        body_text: (*body).into(),
    })
    .collect::<Vec<_>>();
    let placed = store.insert_messages(&msgs).unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for id in &placed {
        store.place_message(*id, inbox).unwrap();
    }

    let best: Vec<String> = store
        .search_threads_sorted("annex", 10, false)
        .unwrap()
        .into_iter()
        .map(|r| r.subject)
        .collect();
    let newest: Vec<String> = store
        .search_threads_sorted("annex", 10, true)
        .unwrap()
        .into_iter()
        .map(|r| r.subject)
        .collect();

    assert_eq!(
        newest,
        ["Newest", "Middle", "Annex"],
        "date order, newest first"
    );
    assert_eq!(
        best.first().map(String::as_str),
        Some("Annex"),
        "ranked, not dated"
    );
    assert_ne!(best, newest, "the toggle has to change something");
}

/// Junk and deleted mail, and whether a search should hand it back.
///
/// A result that quietly comes from Spam is worse than no result: the filter
/// already judged that message, and a search that returns it puts it back in
/// front of the reader looking like ordinary mail.
mod the_bin {
    use super::*;
    use petrel_engine::store::ListView;

    fn subjects(store: &Store, q: &str) -> Vec<String> {
        let mut s: Vec<String> = store
            .search_threads(q, 50)
            .unwrap()
            .into_iter()
            .map(|t| t.subject)
            .collect();
        s.sort();
        s
    }

    #[test]
    fn spam_is_not_a_search_result() {
        let (store, ids) = seeded();
        let account = store.first_account().unwrap().unwrap();
        let spam = store.ensure_folder(account, "spam", "spam").unwrap();
        store.place_message(ids[0], spam).unwrap();

        assert_eq!(subjects(&store, "annex"), vec!["Draft terms for review"]);
    }

    #[test]
    fn trash_is_not_a_search_result_either() {
        let (store, ids) = seeded();
        let account = store.first_account().unwrap().unwrap();
        let trash = store.ensure_folder(account, "trash", "trash").unwrap();
        store.place_message(ids[1], trash).unwrap();

        assert_eq!(subjects(&store, "annex"), vec!["Q3 vendor contracts"]);
    }

    #[test]
    fn asking_for_spam_by_name_searches_it() {
        // The grammar is the way in: nothing else reaches these messages, and
        // a reader who types `in:spam` has said what they want.
        let (store, ids) = seeded();
        let account = store.first_account().unwrap().unwrap();
        let spam = store.ensure_folder(account, "spam", "spam").unwrap();
        store.place_message(ids[0], spam).unwrap();

        assert_eq!(
            subjects(&store, "annex in:spam"),
            vec!["Q3 vendor contracts"]
        );
    }

    #[test]
    fn a_condition_only_search_leaves_the_bin_out_too() {
        // `has:attachment` and friends take a different path through the store
        // than a search with words in it, and the rule has to hold on both.
        let (store, ids) = seeded();
        let account = store.first_account().unwrap().unwrap();
        let spam = store.ensure_folder(account, "spam", "spam").unwrap();
        store.place_message(ids[0], spam).unwrap();
        store
            .set_flags(ids[0], petrel_engine::store::flags::FLAGGED, 0)
            .unwrap();
        store
            .set_flags(ids[1], petrel_engine::store::flags::FLAGGED, 0)
            .unwrap();

        assert_eq!(
            subjects(&store, "is:starred"),
            vec!["Draft terms for review"]
        );
    }

    #[test]
    fn the_spam_view_itself_still_shows_it() {
        let (store, ids) = seeded();
        let account = store.first_account().unwrap().unwrap();
        let spam = store.ensure_folder(account, "spam", "spam").unwrap();
        store.place_message(ids[0], spam).unwrap();

        let rows = store.list_threads(&ListView::parse("spam"), 0, 50).unwrap();
        assert_eq!(rows.len(), 1, "the Spam view is where spam belongs");
    }
}

/// The wall between accounts, at the surface where it was missing.
mod account_walls {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::Store;

    fn message(mid: &str, subject: &str, body: &str, attach: bool) -> Vec<u8> {
        let attachment = if attach {
            "Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBODY\r\n--b\r\nContent-Type: application/pdf; name=\"f.pdf\"\r\nContent-Disposition: attachment; filename=\"f.pdf\"\r\n\r\nx\r\n--b--\r\n".to_string()
        } else {
            format!("Content-Type: text/plain\r\n\r\n{body}\r\n")
        };
        format!(
            "From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\n\
             Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
             Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n{attachment}"
        )
        .replace("BODY", body)
        .into_bytes()
    }

    #[test]
    fn search_answers_only_for_the_account_on_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let a = store.ensure_test_account().unwrap();
        let b = store
            .add_account(
                "imap",
                "other@example.net",
                "Other",
                &petrel_engine::store::AccountServers {
                    imap_host: "imap.example.net".into(),
                    imap_port: 993,
                    smtp_host: "smtp.example.net".into(),
                    smtp_port: 465,
                    username: "other@example.net".into(),
                    provider: String::new(),
                },
            )
            .unwrap();
        let inbox_a = store.ensure_folder(a, "inbox", "INBOX").unwrap();
        let inbox_b = store.ensure_folder(b, "inbox", "INBOX").unwrap();

        // The same word lives in both accounts; an attachment lives only in B.
        store
            .ingest_raw(
                &blobs,
                a,
                Some(inbox_a),
                Some(1),
                &message("a1@x", "mine", "heliotrope invoice", false),
            )
            .unwrap();
        store
            .ingest_raw(
                &blobs,
                b,
                Some(inbox_b),
                Some(1),
                &message("b1@x", "theirs", "heliotrope contract", false),
            )
            .unwrap();
        store
            .ingest_raw(
                &blobs,
                b,
                Some(inbox_b),
                Some(2),
                &message("b2@x", "attached", "files", true),
            )
            .unwrap();

        store.set_active_account(a).unwrap();

        // Text search: the other account's hit must not appear.
        let hits = store.search_threads("heliotrope", 20).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].subject, "mine");

        // Conditions-only search: B's attachment is not A's result.
        let hits = store.search_threads("has:attachment", 20).unwrap();
        assert!(hits.is_empty(), "{hits:?}");

        // And from the other side, both answers flip.
        store.set_active_account(b).unwrap();
        let hits = store.search_threads("heliotrope", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "theirs");
        assert_eq!(store.search_threads("has:attachment", 20).unwrap().len(), 1);
    }
}

/// `in:` names places the user made, not only roles.
mod in_user_folders {
    use petrel_engine::blob::BlobStore;
    use petrel_engine::store::Store;

    #[test]
    fn a_folder_can_be_searched_by_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("t.db")).unwrap();
        let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
        let account = store.ensure_test_account().unwrap();
        let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
        let nested = store
            .ensure_named_folder(account, "Projects/Petrel")
            .unwrap();
        let raw = |mid: &str, body: &str| {
            format!(
                "From: a@example.com\r\nTo: b@example.com\r\nSubject: s\r\n\
                 Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\n{body}\r\n"
            )
            .into_bytes()
        };
        store
            .ingest_raw(
                &blobs,
                account,
                Some(inbox),
                Some(1),
                &raw("i@x", "annex in the inbox"),
            )
            .unwrap();
        store
            .ingest_raw(
                &blobs,
                account,
                Some(nested),
                Some(1),
                &raw("f@x", "annex in the folder"),
            )
            .unwrap();

        // The leaf, case-insensitively, and the full path both address it.
        assert_eq!(
            store.search_threads("in:petrel annex", 20).unwrap().len(),
            1
        );
        assert_eq!(
            store
                .search_threads("in:projects/petrel annex", 20)
                .unwrap()
                .len(),
            1
        );
        // Unscoped still finds both; a role keeps meaning the role.
        assert_eq!(store.search_threads("annex", 20).unwrap().len(), 2);
        assert_eq!(store.search_threads("in:inbox annex", 20).unwrap().len(), 1);
    }
}
