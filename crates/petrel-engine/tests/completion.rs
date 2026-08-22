//! Recipient completion: who gets offered, in what order, and who never does.
//!
//! Everything here comes from mail already synced — there is no lookup
//! anywhere else, which is the point. A composer that asks a server who you
//! might be writing to has just told it who you are writing to.

use petrel_engine::store::{NewMessage, Store};

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
