//! Recipient completion: who gets offered, in what order, and who never does.
//!
//! Everything here comes from mail already synced — there is no lookup
//! anywhere else, which is the point. A composer that asks a server who you
//! might be writing to has just told it who you are writing to.

use std::collections::BTreeMap;

use petrel_engine::store::{
    CorrespondentStats, NewMessage, Store, merge_correspondent_stats, rank_correspondents,
};

const DAY: i64 = 86_400_000;

fn seeded() -> (Store, i64, i64) {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let now = 1_700_000_000_000;
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();
    let spam = store.ensure_folder(account, "spam", "Spam").unwrap();

    // A mailing list that writes constantly and has never been replied to.
    for i in 0..30 {
        let id = store
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: now - i * DAY,
                from_addr: "news@example.com".into(),
                from_display: "News Digest".into(),
                to_addr: "me@example.com".into(),
                subject: format!("issue {i}"),
                body_text: "…".into(),
            }])
            .unwrap()[0];
        store.place_message(id, inbox).unwrap();
    }

    // One person, written to once, recently.
    let out = store
        .insert_messages(&[NewMessage {
            account_id: account,
            date_ms: now - DAY,
            from_addr: "me@example.com".into(),
            from_display: "Me".into(),
            to_addr: "nadia@example.com".into(),
            subject: "lunch".into(),
            body_text: "…".into(),
        }])
        .unwrap()[0];
    store.place_message(out, sent).unwrap();

    // And a spammer, who is loud.
    for i in 0..10 {
        let id = store
            .insert_messages(&[NewMessage {
                account_id: account,
                date_ms: now - i,
                from_addr: "newmoney@example.com".into(),
                from_display: "New Offer".into(),
                to_addr: "me@example.com".into(),
                subject: "win".into(),
                body_text: "…".into(),
            }])
            .unwrap()[0];
        store.place_message(id, spam).unwrap();
    }
    (store, account, now)
}

fn addrs(store: &Store, account: i64, now: i64, prefix: &str) -> Vec<String> {
    store
        .complete_addresses(account, prefix, now, 10)
        .unwrap()
        .into_iter()
        .map(|c| c.addr)
        .collect()
}

#[test]
fn someone_written_to_outranks_a_mailing_list_that_writes_constantly() {
    let (store, account, now) = seeded();
    let out = addrs(&store, account, now, "n");
    assert_eq!(
        out.first().map(String::as_str),
        Some("nadia@example.com"),
        "thirty newsletters should not bury the person you emailed yesterday: {out:?}"
    );
}

#[test]
fn spam_is_never_offered() {
    let (store, account, now) = seeded();
    let out = addrs(&store, account, now, "n");
    assert!(
        !out.iter().any(|a| a.contains("newmoney")),
        "a spammer got into the completion list: {out:?}"
    );
}

#[test]
fn your_own_address_is_not_a_suggestion() {
    let (store, account, now) = seeded();
    let own = store.accounts().unwrap()[0].email.to_lowercase();
    let out = addrs(&store, account, now, &own[..2]);
    assert!(
        !out.contains(&own),
        "offered to send mail to yourself: {out:?}"
    );
}

#[test]
fn matches_the_start_of_a_name_a_domain_or_the_address() {
    let (store, account, now) = seeded();
    assert!(addrs(&store, account, now, "nadia").contains(&"nadia@example.com".to_string()));
    assert!(addrs(&store, account, now, "example.com").contains(&"nadia@example.com".to_string()));
    assert!(
        addrs(&store, account, now, "News").contains(&"news@example.com".to_string()),
        "a display name should match however it was capitalised"
    );
}

#[test]
fn an_empty_prefix_offers_nothing() {
    let (store, account, now) = seeded();
    assert!(addrs(&store, account, now, "").is_empty());
    assert!(addrs(&store, account, now, "   ").is_empty());
}

fn person(addr: &str, display: &str, written: bool, seen: i64, last_ms: i64) -> CorrespondentStats {
    CorrespondentStats {
        addr: addr.into(),
        display: display.into(),
        names: vec![display.into()],
        written,
        seen,
        last_ms,
    }
}

fn ranked(stats: &[CorrespondentStats], prefix: &str, now: i64, own: &str) -> Vec<String> {
    rank_correspondents(stats, prefix, now, own, 10)
        .into_iter()
        .map(|c| c.addr)
        .collect()
}

#[test]
fn rank_offers_nothing_for_an_empty_or_blank_prefix() {
    let now = 1_700_000_000_000;
    let stats = vec![person("nadia@example.com", "Nadia Okafor", true, 1, now)];
    assert!(ranked(&stats, "", now, "").is_empty());
    assert!(ranked(&stats, "   ", now, "").is_empty());
    assert!(ranked(&stats, "\n\t", now, "").is_empty());
}

#[test]
fn rank_matches_address_domain_and_display_starts() {
    let now = 1_700_000_000_000;
    // The digest address does not start with "news", so that prefix can only
    // hit the display. "example.com" is the domain, not a prefix of the address.
    let stats = vec![
        person("nadia@example.com", "Nadia Okafor", false, 1, now),
        person("digest@letters.test", "News Digest", false, 2, now),
    ];
    assert_eq!(
        ranked(&stats, "nadia", now, ""),
        vec!["nadia@example.com".to_string()]
    );
    assert_eq!(
        ranked(&stats, "example.com", now, ""),
        vec!["nadia@example.com".to_string()]
    );
    // A fragment of the domain is not a domain start. `example.com` does not
    // begin with `ample.com`.
    assert!(ranked(&stats, "ample.com", now, "").is_empty());
    assert_eq!(
        ranked(&stats, "News", now, ""),
        vec!["digest@letters.test".to_string()],
        "a display name should match however it was capitalised"
    );
    assert_eq!(
        ranked(&stats, "okafor", now, ""),
        vec!["nadia@example.com".to_string()],
        "a word inside the display name is a start, not a substring of the address"
    );
}

#[test]
fn rank_does_not_match_a_fragment_in_the_middle_of_a_surname() {
    let now = 1_700_000_000_000;
    let stats = vec![person("sam@example.com", "Sam Anand", false, 1, now)];
    assert_eq!(
        ranked(&stats, "an", now, ""),
        vec!["sam@example.com".to_string()],
        "the start of a display word still matches"
    );
    assert!(
        ranked(&stats, "nan", now, "").is_empty(),
        "nan sits inside Anand and is not the start of the name, a word, the address, or the domain"
    );
}

#[test]
fn rank_puts_one_sent_message_ahead_of_a_frequent_newsletter() {
    let now = 1_700_000_000_000;
    // The newsletter is newer and far past the frequency cap. Written-to is
    // the first key, so one old sent message still comes first.
    let stats = vec![
        person("news@example.com", "News Digest", false, 30, now),
        person("nadia@example.com", "Nadia", true, 1, now - 400 * DAY),
    ];
    let out = rank_correspondents(&stats, "n", now, "", 10);
    assert_eq!(
        out.first().map(|c| c.addr.as_str()),
        Some("nadia@example.com")
    );
    assert!(out.first().is_some_and(|c| c.written_to));
}

#[test]
fn rank_drops_the_accounts_own_address() {
    let now = 1_700_000_000_000;
    // The fixture never writes the account address onto a message, so the
    // integration test can pass without this filter. The row has to be here.
    let stats = vec![
        person("test@example.com", "Test Account", true, 9, now),
        person("tina@example.com", "Tina", false, 1, now - DAY),
    ];
    let out = ranked(&stats, "t", now, "  Test@Example.com  ");
    assert!(!out.iter().any(|a| a == "test@example.com"), "{out:?}");
    assert_eq!(out, vec!["tina@example.com".to_string()]);
}

#[test]
fn a_spam_only_address_is_absent_from_the_load() {
    let (store, account, _) = seeded();
    let stats = store.load_correspondent_stats(account).unwrap();
    assert!(
        stats.iter().all(|s| s.addr != "newmoney@example.com"),
        "a spam-only address was aggregated: {stats:?}"
    );
    assert!(stats.iter().any(|s| s.addr == "nadia@example.com"));
}

fn note(
    account: i64,
    date_ms: i64,
    from_addr: &str,
    from_display: &str,
    to_addr: &str,
) -> NewMessage {
    NewMessage {
        account_id: account,
        date_ms,
        from_addr: from_addr.into(),
        from_display: from_display.into(),
        to_addr: to_addr.into(),
        subject: "note".into(),
        body_text: "…".into(),
    }
}

fn place(store: &mut Store, folder: i64, message: NewMessage) {
    let id = store.insert_messages(&[message]).unwrap()[0];
    store.place_message(id, folder).unwrap();
}

fn by_addr(rows: &[CorrespondentStats]) -> BTreeMap<&str, &CorrespondentStats> {
    rows.iter().map(|s| (s.addr.as_str(), s)).collect()
}

#[test]
fn incremental_merge_matches_a_fresh_full_load() {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let now = 1_700_000_000_000;
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let sent = store.ensure_folder(account, "sent", "Sent").unwrap();

    // Base snapshot. Ada's display is the earlier, lexicographically smaller name.
    place(
        &mut store,
        inbox,
        note(
            account,
            now - 10 * DAY,
            "ada@example.com",
            "Ada",
            "me@example.com",
        ),
    );
    place(
        &mut store,
        inbox,
        note(
            account,
            now - 2 * DAY,
            "news@example.com",
            "News Digest",
            "me@example.com",
        ),
    );
    let after_id = store.max_message_id(account).unwrap();
    let mut merged = store.load_correspondent_stats(account).unwrap();

    // Delta: a later name for Ada, one sent copy (the only `written`), and a new person.
    // The to-row display is empty; the from-row is what carries a name.
    place(
        &mut store,
        inbox,
        note(
            account,
            now - DAY,
            "ada@example.com",
            "Zoe",
            "me@example.com",
        ),
    );
    place(
        &mut store,
        sent,
        note(account, now, "me@example.com", "Me", "ada@example.com"),
    );
    place(
        &mut store,
        inbox,
        note(
            account,
            now - 3 * DAY,
            "priya@example.com",
            "Priya Shah",
            "me@example.com",
        ),
    );
    let delta = store
        .load_correspondent_stats_since(account, after_id)
        .unwrap();
    merge_correspondent_stats(&mut merged, delta);
    let fresh = store.load_correspondent_stats(account).unwrap();

    let merged_by = by_addr(&merged);
    let ada = merged_by["ada@example.com"];
    assert_eq!(ada.seen, 3, "two from-rows and the sent to-row");
    assert_eq!(ada.last_ms, now);
    assert!(ada.written, "the sent copy is what sets written");
    assert_eq!(
        ada.display, "Zoe",
        "the non-empty name that sorts higher wins"
    );

    assert_eq!(
        merged.len(),
        fresh.len(),
        "merge and a full load should have the same addresses"
    );
    assert_eq!(by_addr(&merged), by_addr(&fresh));
}

#[test]
fn every_name_an_address_has_been_seen_under_still_finds_it() {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let now = 1_700_000_000_000;
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    // One person, two names: what their own client calls them, and what
    // someone else's calls them. "Mom" sorts after "Jane Doe", so it is the
    // one kept for display. "doe" must still find her: when every keystroke
    // asked the database, the WHERE clause saw each message's name.
    place(
        &mut store,
        inbox,
        note(
            account,
            now - DAY,
            "jane@example.com",
            "Jane Doe",
            "me@example.com",
        ),
    );
    place(
        &mut store,
        inbox,
        note(account, now, "jane@example.com", "Mom", "me@example.com"),
    );

    let found = store.complete_addresses(account, "doe", now, 10).unwrap();
    assert_eq!(
        found.iter().map(|c| c.addr.as_str()).collect::<Vec<_>>(),
        vec!["jane@example.com"],
        "a name the address was seen under is a name it can be found by"
    );
    assert_eq!(
        store
            .complete_addresses(account, "mom", now, 10)
            .unwrap()
            .len(),
        1
    );
}
