//! Preferences are stored separately from Petrel's own bookkeeping, and a
//! missing preference means "use the default" rather than "store the default".

use petrel_engine::store::Store;

#[test]
fn settings_round_trip_and_overwrite() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.settings().unwrap().is_empty(), "nothing is set to begin with");

    store.set_setting("theme", "dark").unwrap();
    store.set_setting("density", "compact").unwrap();
    assert_eq!(store.settings().unwrap().get("theme").map(String::as_str), Some("dark"));

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
    store.set_setting("extractor_version", "user nonsense").unwrap();

    assert_eq!(store.meta("extractor_version").unwrap().as_deref(), Some("9"));
    assert_eq!(
        store.settings().unwrap().get("extractor_version").map(String::as_str),
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
        store.settings().unwrap().get("language").map(String::as_str),
        Some("de"),
        "a preference that does not outlive the process is not a preference"
    );
}
