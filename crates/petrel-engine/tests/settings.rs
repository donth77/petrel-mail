//! Preferences are stored separately from Petrel's own bookkeeping, and a
//! missing preference means "use the default" rather than "store the default".

use petrel_engine::store::Store;

#[test]
fn settings_round_trip_and_overwrite() {
    let store = Store::open_in_memory().unwrap();
    assert!(
        store.settings().unwrap().is_empty(),
        "nothing is set to begin with"
    );

    store.set_setting("theme", "dark").unwrap();
    store.set_setting("density", "compact").unwrap();
    assert_eq!(
        store.settings().unwrap().get("theme").map(String::as_str),
        Some("dark")
    );

    store.set_setting("theme", "light").unwrap();
    assert_eq!(
        store.settings().unwrap().get("theme").map(String::as_str),
        Some("light"),
        "setting the same key replaces rather than duplicates"
    );
    assert_eq!(store.settings().unwrap().len(), 2);
}

#[test]
fn clearing_a_setting_removes_it_rather_than_storing_a_default() {
    let store = Store::open_in_memory().unwrap();
    store.set_setting("accent", "#0E7C86").unwrap();
    store.clear_setting("accent").unwrap();

    // Absent, not "#0E7C86" — so if the default accent ever changes, a user who
    // never chose one moves with it instead of being pinned to the old value.
    assert!(!store.settings().unwrap().contains_key("accent"));
}

#[test]
fn preferences_do_not_collide_with_engine_bookkeeping() {
    let store = Store::open_in_memory().unwrap();
    store.set_meta("extractor_version", "9").unwrap();
    store
        .set_setting("extractor_version", "user nonsense")
        .unwrap();

    assert_eq!(
        store.meta("extractor_version").unwrap().as_deref(),
        Some("9")
    );
    assert_eq!(
        store
            .settings()
            .unwrap()
            .get("extractor_version")
            .map(String::as_str),
        Some("user nonsense"),
        "same key name in both tables must not clobber the engine's value"
    );
}

#[test]
fn settings_survive_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("petrel.db");
    {
        let store = Store::open(&path).unwrap();
        store.set_setting("language", "de").unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .settings()
            .unwrap()
            .get("language")
            .map(String::as_str),
        Some("de"),
        "a preference that does not outlive the process is not a preference"
    );
}

#[test]
fn account_summary_carries_counts_and_folder_mapping() {
    use petrel_engine::store::{NewMessage, flags};

    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    store.set_account_color(account, "#9A6B1F").unwrap();

    let msgs: Vec<NewMessage> = (0..5)
        .map(|i| NewMessage {
            account_id: account,
            date_ms: 1_000 + i,
            from_addr: "a@example.com".into(),
            from_display: "A".into(),
            to_addr: "me@example.com".into(),
            subject: format!("m{i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    for id in ids.iter().take(3) {
        store.set_flags(*id, flags::SEEN, 0).unwrap();
    }
    // The header's unread is the inbox's unread — the number every other
    // surface shows. Four of five live there; the fifth, unread but filed
    // away, is true but not this number's business.
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    for (i, id) in ids.iter().enumerate() {
        if i < 4 {
            store.place_message_at(*id, inbox, (i as u32) + 1).unwrap();
        }
    }

    let accounts = store.accounts().unwrap();
    let a = accounts
        .iter()
        .find(|a| a.id == account)
        .expect("the account");

    assert_eq!(a.message_count, 5);
    assert_eq!(
        a.unread_count, 1,
        "three read, one unread in the inbox, one unread filed away"
    );
    assert_eq!(a.color, "#9A6B1F");
    assert_eq!(
        a.newest_ms,
        Some(1_004),
        "newest message stands in for last sync"
    );
    assert!(!a.local_archive, "mirror is the default (Q24)");
}
