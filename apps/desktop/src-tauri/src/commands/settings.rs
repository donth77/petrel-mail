//! App-level state and preferences: status for the status bar, settings, and filter rules.

use crate::config::imap_config_from_env;
use crate::state::{AppState, Timed, active_account};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::State;

#[derive(serde::Serialize)]
pub(crate) struct Status {
    /// Whether any account can sign in — set up in the app, or given by the
    /// environment. `false` is the first-run signal: the window shows
    /// onboarding instead of an empty mailbox pretending to be a mailbox.
    configured: bool,
    /// Showing synthetic mail with no account configured. Distinct from
    /// `configured`: both are false on a first run, but only one of them
    /// means "there is a mailbox here to look at".
    demo: bool,
    seeding: bool,
    count: usize,
    /// What the server says it holds across the synced folders, or 0 if it has
    /// not been asked yet.
    server_total: usize,
    source: String,
    /// The retention mode, in words. Q24's binding rule is that the active
    /// policy is always stated — never something the user has to infer.
    retention: String,
    data_dir: String,
    sync_error: Option<String>,
    last_sync_ms: i64,
    /// Arrivals a rule marked notify-anyway, drained on read: mail a rule
    /// filed away never reaches the inbox list the announcer watches, so
    /// the rule's word rides the status poll instead. Each entry is said
    /// once, by whichever poll picks it up.
    notify: Vec<(String, String)>,
    /// Things a background worker needs the person to know, by key, drained
    /// the same way. A worker has no words of its own — the window owns
    /// those, and translates them — so it raises a key and the window says
    /// the sentence. `sent-copy-failed` is the first: the message went, the
    /// copy in Sent did not.
    alerts: Vec<String>,
}

#[tauri::command(async)]
pub fn status(state: State<Arc<AppState>>) -> Status {
    let _t = Timed::new("status");
    let notify = state
        .pending_notify
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default();
    let alerts = state
        .pending_alerts
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default();
    let (configured, count, retention) = match state.store_read() {
        Ok(s) => {
            let account = s.active_account().ok().flatten();
            // Presence of stored servers, deliberately not a password
            // read: this runs on every status poll, and a keychain read
            // here meant a consent dialog every few seconds on unsigned
            // dev builds.
            let configured = account
                .and_then(|a| {
                    s.account_servers(a)
                        .ok()
                        .flatten()
                        .map(|v| !v.imap_host.is_empty())
                })
                .unwrap_or(false)
                || imap_config_from_env().is_some();
            // The active account's held mail, not the store's total: while one
            // account backfills a deep archive, the other's empty folders were
            // announcing thousands of messages that belonged next door. The
            // global `seeded` counter stays what it is — an internal
            // change-signal — and stops being shown as if it were a fact about
            // whatever account is on screen.
            let count = account
                .and_then(|a| s.message_count_for(a).ok())
                .map(|n| n as usize)
                .unwrap_or(0);
            state.status_count.store(count, Ordering::Relaxed);
            // The account on screen, not the one this launch happened to start
            // with. Retention is per account — one may keep server deletions
            // locally and the other not — and the status bar states the active
            // policy, so naming the wrong account's is worse than saying nothing.
            let retention = s
                .retention_mode(account.unwrap_or(state.account_id))
                .ok()
                .map(|m| m.describe().to_string())
                .unwrap_or_default();
            (configured, count, retention)
        }
        Err(_) => (
            imap_config_from_env().is_some(),
            state.status_count.load(Ordering::Relaxed),
            String::new(),
        ),
    };
    Status {
        configured,
        demo: state.demo.load(Ordering::Relaxed),
        notify,
        alerts,
        seeding: state.seeding.load(Ordering::Relaxed),
        count,
        server_total: state.server_total.load(Ordering::Relaxed),
        last_sync_ms: state.last_sync_ms.load(Ordering::Relaxed),
        source: state
            .source
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "unknown".into()),
        retention,
        data_dir: state.data_dir.clone(),
        sync_error: state.sync_error.lock().ok().and_then(|e| e.clone()),
    }
}

#[tauri::command(async)]
pub fn get_settings(state: State<Arc<AppState>>) -> Result<HashMap<String, String>, String> {
    let store = state.store()?;
    store.settings().map_err(|e| e.to_string())
}

/// An empty value clears the preference, restoring the default rather than
/// pinning the current one.
#[tauri::command(async)]
pub fn set_setting(key: String, value: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    if value.is_empty() {
        store.clear_setting(&key).map_err(|e| e.to_string())
    } else {
        store.set_setting(&key, &value).map_err(|e| e.to_string())
    }
}

/// The active account's filter rules, in run order.
#[tauri::command(async)]
pub fn list_rules(state: State<Arc<AppState>>) -> Result<Vec<petrel_engine::rules::Rule>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.rules_for_account(account).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn save_rule(
    rule_id: Option<i64>,
    name: String,
    enabled: bool,
    conditions: Vec<petrel_engine::rules::Condition>,
    actions: petrel_engine::rules::Actions,
    state: State<Arc<AppState>>,
) -> Result<i64, String> {
    let mut store = state.store()?;
    let account = active_account(&store)?;
    store
        .save_rule(account, rule_id, &name, enabled, &conditions, &actions)
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn delete_rule(rule_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.delete_rule(rule_id).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn move_rule(rule_id: i64, up: bool, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.move_rule(rule_id, up).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- backup ---

/// Engine bookkeeping that lives in the settings table but is not a
/// preference: sync watermarks and one-time markers. They describe *this*
/// store's conversation with *these* servers, and carrying them to another
/// machine would hand it a stranger's place-markers. `store_id` is the
/// keychain namespace: imported, it re-keyed every password lookup to the
/// exporting store's name, and each account showed as configured while
/// never syncing again.
const BOOKKEEPING: &[&str] = &[
    "gmail_labels_modseq",
    "gmail_thrid_modseq",
    "keychain_reowned",
    "store_id",
];

#[derive(serde::Serialize, serde::Deserialize)]
pub struct IdentityExport {
    pub display_name: String,
    pub signature: String,
    pub signature_on_reply: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct AccountExport {
    pub email: String,
    pub kind: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub local_archive: bool,
    #[serde(default)]
    pub servers: Option<petrel_engine::store::AccountServers>,
    #[serde(default)]
    pub identity: Option<IdentityExport>,
}

/// The whole backup: preferences and account shapes, and pointedly nothing
/// secret. Passwords live in the keychain and are never written here — the
/// UI says so in as many words beside the button.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SettingsFile {
    pub version: u32,
    pub exported_at_ms: i64,
    pub settings: std::collections::BTreeMap<String, String>,
    pub accounts: Vec<AccountExport>,
}

fn build_settings_file(store: &petrel_engine::store::Store) -> Result<SettingsFile, String> {
    let settings = store
        .settings()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|(k, _)| !BOOKKEEPING.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut accounts = Vec::new();
    for a in store.accounts().map_err(|e| e.to_string())? {
        let servers = store.account_servers(a.id).ok().flatten();
        let identity = store.identity(a.id).ok().map(|i| IdentityExport {
            display_name: i.display_name,
            signature: i.signature,
            signature_on_reply: i.signature_on_reply,
        });
        accounts.push(AccountExport {
            email: a.email,
            kind: a.kind,
            color: a.color,
            local_archive: a.local_archive,
            servers,
            identity,
        });
    }
    Ok(SettingsFile {
        version: 1,
        exported_at_ms: crate::state::now_ms(),
        settings,
        accounts,
    })
}

/// Applies a backup by merging: file entries land, everything else stands.
/// Returns (settings applied, accounts updated, accounts added).
fn apply_settings_file(
    store: &petrel_engine::store::Store,
    file: &SettingsFile,
) -> Result<(usize, usize, usize), String> {
    let mut applied = 0usize;
    for (k, v) in &file.settings {
        if BOOKKEEPING.contains(&k.as_str()) {
            continue;
        }
        store.set_setting(k, v).map_err(|e| e.to_string())?;
        applied += 1;
    }
    let existing = store.accounts().map_err(|e| e.to_string())?;
    let (mut updated, mut added) = (0usize, 0usize);
    for imp in &file.accounts {
        let found = existing
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(&imp.email));
        let id = match found {
            Some(a) => {
                updated += 1;
                a.id
            }
            None => {
                added += 1;
                store
                    .add_account(
                        &imp.kind,
                        &imp.email,
                        imp.identity
                            .as_ref()
                            .map(|i| i.display_name.as_str())
                            .unwrap_or(""),
                        &imp.servers.clone().unwrap_or_default(),
                    )
                    .map_err(|e| e.to_string())?
            }
        };
        // Servers only for an account this store did not already have. A
        // backup carries the *exporting* machine's server settings, and
        // applying them to an account already set up here re-points a
        // working mailbox at whatever that file says — a stale host, a
        // wrong port, or a username that signs in to somebody else's mail.
        // Preferences merge; a live account's connection does not.
        if let (Some(servers), None) = (&imp.servers, found) {
            store
                .set_account_servers(id, servers)
                .map_err(|e| e.to_string())?;
        }
        if !imp.color.is_empty() {
            store
                .set_account_color(id, &imp.color)
                .map_err(|e| e.to_string())?;
        }
        store
            .set_local_archive(id, imp.local_archive)
            .map_err(|e| e.to_string())?;
        if let Some(i) = &imp.identity {
            store
                .set_identity(id, &i.display_name, &i.signature, i.signature_on_reply)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok((applied, updated, added))
}

/// Writes the settings backup to a path the user picked.
#[tauri::command(async)]
pub fn export_settings(path: String, state: State<Arc<AppState>>) -> Result<String, String> {
    // Where the save panel said, and nowhere else.
    let target = state.vetted_path(&path, &[])?;
    let store = state.store()?;
    let file = build_settings_file(&store)?;
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&target, json).map_err(|e| e.to_string())?;
    Ok(format!("{}/{}", file.settings.len(), file.accounts.len()))
}

/// Merges a settings backup in. Never deletes: an account or preference the
/// file does not mention is left exactly as it was. Accounts the file adds
/// arrive without passwords — those stay in the keychain of the machine
/// that wrote the file — and sync once one is entered.
#[tauri::command(async)]
pub fn import_settings(path: String, state: State<Arc<AppState>>) -> Result<String, String> {
    let source = state.vetted_path(&path, &[])?;
    let raw = std::fs::read_to_string(&source).map_err(|e| e.to_string())?;
    let file: SettingsFile = serde_json::from_str(&raw)
        .map_err(|_| "This is not a Petrel settings file.".to_string())?;
    if file.version > 1 {
        return Err("This file was written by a newer Petrel.".into());
    }
    let store = state.store()?;
    let (applied, updated, added) = apply_settings_file(&store, &file)?;
    Ok(format!("{applied}/{updated}/{added}"))
}

#[cfg(test)]
mod backup_tests {
    use super::{SettingsFile, apply_settings_file, build_settings_file};
    use petrel_engine::store::Store;

    #[test]
    fn a_file_that_carries_a_store_id_does_not_rekey_the_keychain() {
        // A file from an older build, or edited by hand, that names the
        // exporting store's keychain namespace. Applying it must leave this
        // store's own, or every password lookup here fails from the next
        // launch on.
        let a = Store::open_in_memory().unwrap();
        a.set_setting("theme", "dark").unwrap();
        let mut file = build_settings_file(&a).unwrap();
        file.settings
            .insert("store_id".to_string(), "store-a".to_string());
        let b = Store::open_in_memory().unwrap();
        b.set_setting("store_id", "store-b").unwrap();
        apply_settings_file(&b, &file).unwrap();
        let s = b.settings().unwrap();
        assert_eq!(s.get("store_id").map(String::as_str), Some("store-b"));
        assert_eq!(s.get("theme").map(String::as_str), Some("dark"));
    }

    /// An account already configured here keeps the servers it is signing
    /// in with. A backup taken from another machine — or an older one taken
    /// from this machine before a host changed — used to overwrite them,
    /// and the account stopped syncing with nothing on screen to say why.
    #[test]
    fn importing_never_repoints_an_account_that_is_already_set_up() {
        use petrel_engine::store::AccountServers;

        let a = Store::open_in_memory().unwrap();
        a.add_account(
            "imap",
            "me@example.com",
            "Me",
            &AccountServers {
                imap_host: "old.example.com".into(),
                imap_port: 993,
                smtp_host: "smtp.old.example.com".into(),
                smtp_port: 465,
                username: "me@example.com".into(),
                provider: String::new(),
            },
        )
        .unwrap();
        let file = build_settings_file(&a).unwrap();

        let b = Store::open_in_memory().unwrap();
        let live = b
            .add_account(
                "imap",
                "me@example.com",
                "Me",
                &AccountServers {
                    imap_host: "current.example.com".into(),
                    imap_port: 993,
                    smtp_host: "smtp.current.example.com".into(),
                    smtp_port: 587,
                    username: "me".into(),
                    provider: String::new(),
                },
            )
            .unwrap();
        apply_settings_file(&b, &file).unwrap();
        let servers = b.account_servers(live).unwrap().unwrap();
        assert_eq!(
            servers.imap_host, "current.example.com",
            "an account already set up here kept its own servers"
        );
        assert_eq!(servers.username, "me");

        // An account the file adds does get the servers it names — that is
        // the whole use of carrying them.
        let c = Store::open_in_memory().unwrap();
        apply_settings_file(&c, &file).unwrap();
        let added = c.accounts().unwrap()[0].id;
        assert_eq!(
            c.account_servers(added).unwrap().unwrap().imap_host,
            "old.example.com"
        );
    }

    #[test]
    fn a_backup_round_trips_and_merges_rather_than_clobbers() {
        let a = Store::open_in_memory().unwrap();
        let acc = a.ensure_test_account().unwrap();
        a.set_setting("theme", "dark").unwrap();
        a.set_setting("gmail_thrid_modseq", "99").unwrap();
        a.set_setting("store_id", "store-a").unwrap();
        a.set_account_color(acc, "#9A6B1F").unwrap();
        a.set_identity(acc, "Tom", "— t", true).unwrap();
        let file = build_settings_file(&a).unwrap();
        assert_eq!(file.settings.get("theme").map(String::as_str), Some("dark"));
        assert!(
            !file.settings.contains_key("gmail_thrid_modseq"),
            "bookkeeping stays home"
        );
        assert!(
            !file.settings.contains_key("store_id"),
            "the keychain namespace stays home"
        );
        assert_eq!(file.accounts.len(), 1);

        // A second store with its own preference the file never mentions.
        let b = Store::open_in_memory().unwrap();
        b.set_setting("badges", "unread").unwrap();
        let (applied, updated, added) = apply_settings_file(&b, &file).unwrap();
        assert_eq!((updated, added), (0, 1), "the account is new here");
        assert!(applied >= 1);
        let s = b.settings().unwrap();
        assert_eq!(s.get("theme").map(String::as_str), Some("dark"));
        assert_eq!(
            s.get("badges").map(String::as_str),
            Some("unread"),
            "merging never clobbers what the file does not mention"
        );
        let accs = b.accounts().unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].color, "#9A6B1F");

        // Importing the same file again updates rather than duplicates.
        let (_, updated2, added2) = apply_settings_file(&b, &file).unwrap();
        assert_eq!((updated2, added2), (1, 0));

        // A newer version is refused before anything is touched — checked at
        // the command layer; the struct itself still parses.
        let newer = SettingsFile {
            version: 2,
            ..serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap()
        };
        assert_eq!(newer.version, 2);
    }
}

/// Posts a desktop notification, and reports whether it actually happened.
///
/// A separate command rather than the notification plugin's, because on macOS
/// the plugin's path is a dead end that returns success — see `crate::notify`.
/// Windows and Linux still go through the plugin, from the frontend, where the
/// OS-level story is sound.
///
/// The return value exists so the settings pane's test button can say
/// something true. Silence was indistinguishable from success, which is the
/// one thing a button called "send a test notification" must never be.
///
/// Async, and off the UI thread, because the first call is the one that puts
/// the system's permission prompt on screen. A sync command runs on the main
/// thread, and waiting there for an answer the user has not given yet freezes
/// the window behind the very dialog they need to read.
#[tauri::command]
pub async fn post_notification(title: String, body: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::notify::post(&title, &body))
        .await
        .map_err(|e| e.to_string())?
}
