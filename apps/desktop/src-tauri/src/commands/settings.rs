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
}

#[tauri::command]
pub fn status(state: State<Arc<AppState>>) -> Status {
    let _t = Timed::new("status");
    let configured = state
        .store
        .lock()
        .ok()
        .and_then(|s| {
            s.active_account().ok().flatten().and_then(|a| {
                // Presence of stored servers, deliberately not a password
                // read: this runs on every status poll, and a keychain read
                // here meant a consent dialog every few seconds on unsigned
                // dev builds.
                s.account_servers(a)
                    .ok()
                    .flatten()
                    .map(|v| !v.imap_host.is_empty())
            })
        })
        .unwrap_or(false)
        || imap_config_from_env().is_some();
    Status {
        configured,
        seeding: state.seeding.load(Ordering::Relaxed),
        // The active account's held mail, not the store's total: while one
        // account backfills a deep archive, the other's empty folders were
        // announcing thousands of messages that belonged next door. The
        // global `seeded` counter stays what it is — an internal
        // change-signal — and stops being shown as if it were a fact about
        // whatever account is on screen.
        count: state
            .store
            .lock()
            .ok()
            .and_then(|s| {
                s.active_account()
                    .ok()
                    .flatten()
                    .and_then(|a| s.message_count_for(a).ok())
            })
            .map(|n| n as usize)
            .unwrap_or_else(|| state.seeded.load(Ordering::Relaxed)),
        server_total: state.server_total.load(Ordering::Relaxed),
        last_sync_ms: state.last_sync_ms.load(Ordering::Relaxed),
        source: state
            .source
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "unknown".into()),
        retention: state
            .store
            .lock()
            .ok()
            .and_then(|s| s.retention_mode(state.account_id).ok())
            .map(|m| m.describe().to_string())
            .unwrap_or_default(),
        data_dir: state.data_dir.clone(),
        sync_error: state.sync_error.lock().ok().and_then(|e| e.clone()),
    }
}

#[tauri::command]
pub fn get_settings(state: State<Arc<AppState>>) -> Result<HashMap<String, String>, String> {
    let store = state.store()?;
    store.settings().map_err(|e| e.to_string())
}

/// An empty value clears the preference, restoring the default rather than
/// pinning the current one.
#[tauri::command]
pub fn set_setting(key: String, value: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    if value.is_empty() {
        store.clear_setting(&key).map_err(|e| e.to_string())
    } else {
        store.set_setting(&key, &value).map_err(|e| e.to_string())
    }
}

/// The active account's filter rules, in run order.
#[tauri::command]
pub fn list_rules(state: State<Arc<AppState>>) -> Result<Vec<petrel_engine::rules::Rule>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.rules_for_account(account).map_err(|e| e.to_string())
}

#[tauri::command]
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

#[tauri::command]
pub fn delete_rule(rule_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.delete_rule(rule_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn move_rule(rule_id: i64, up: bool, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.move_rule(rule_id, up).map_err(|e| e.to_string())
}
