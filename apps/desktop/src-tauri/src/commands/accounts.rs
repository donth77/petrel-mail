//! Accounts: discovering a server, testing it, adding and removing accounts, and choosing the one on screen.

use crate::config::{imap_config, imap_config_from_env, keychain_entry, remember_password};
use crate::state::{AppState, note_ui_touch};
use crate::sync::spawn_real_sync;
use petrel_engine::store::AccountSummary;
use petrel_providers::imap::{Credential, ImapConfig, Security};
use std::sync::Arc;
use tauri::State;

/// Step 1 → 2 of onboarding: what an address tells us about its servers.
#[tauri::command]
pub async fn discover_account(
    address: String,
) -> Result<Option<petrel_autoconfig::Discovered>, String> {
    petrel_autoconfig::discover(&address)
        .await
        .map_err(|e| e.to_string())
}

/// The manual form's pre-fill when nothing answered: the conventional hosts.
#[tauri::command]
pub fn guess_servers(
    address: String,
) -> Option<(petrel_autoconfig::Server, petrel_autoconfig::Server)> {
    petrel_autoconfig::guess(&address)
}

#[derive(serde::Deserialize)]
pub(crate) struct AccountSetup {
    email: String,
    username: String,
    password: String,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
    provider: String,
}

/// "Reached both servers over TLS. Certificates check out." — or why not.
///
/// Runs before anything is stored. The two halves are reported separately so
/// the form can say which server is wrong rather than "something failed".
#[tauri::command]
pub async fn test_account(setup: AccountSetup, which: Option<String>) -> Result<(), String> {
    // On a spawned task rather than the command's own future. Tauri drives
    // async commands from its own runtime, and a TLS handshake — which
    // builds a root store and blocks on the socket — run inline there stalled
    // without ever resolving. Spawned, it runs where the sync already does.
    tauri::async_runtime::spawn(test_account_inner(setup, which))
        .await
        .map_err(|e| format!("test task: {e}"))?
}

/// `which` is "imap", "smtp", or absent for both in turn. Split so the form
/// can report each half as it happens: some providers take several seconds
/// per login, and one spinner over both reads as stuck halfway through.
async fn test_account_inner(setup: AccountSetup, which: Option<String>) -> Result<(), String> {
    let do_imap = which.as_deref() != Some("smtp");
    let do_smtp = which.as_deref() != Some("imap");
    let imap = ImapConfig {
        host: setup.imap_host.clone(),
        port: setup.imap_port,
        user: setup.username.clone(),
        credential: Credential::password(setup.password.clone()),
        security: Security::Tls,
    };
    if do_imap {
        petrel_providers::imap::login_check(&imap)
            .await
            .map_err(|e| format!("Incoming (IMAP) — {e}"))?;
    }
    let smtp = petrel_providers::smtp::SmtpConfig {
        host: setup.smtp_host.clone(),
        port: setup.smtp_port,
        user: setup.username.clone(),
        credential: Credential::password(setup.password.clone()),
    };
    if do_smtp {
        petrel_providers::smtp::login_check(&smtp)
            .await
            .map_err(|e| format!("Outgoing (SMTP) — {e}"))?;
    }
    Ok(())
}

/// Stores the account: servers on the row, password in the keychain, and
/// then starts syncing it. Only ever called after `test_account` passed, so
/// a wrong password never reaches the keychain.
#[tauri::command]
pub fn add_account(setup: AccountSetup, state: State<Arc<AppState>>) -> Result<i64, String> {
    let servers = petrel_engine::store::AccountServers {
        imap_host: setup.imap_host,
        imap_port: setup.imap_port,
        smtp_host: setup.smtp_host,
        smtp_port: setup.smtp_port,
        username: setup.username,
        provider: setup.provider.clone(),
    };
    let kind = if setup.provider.to_ascii_lowercase().contains("gmail")
        || setup.provider.to_ascii_lowercase().contains("google")
    {
        "gmail"
    } else {
        "imap"
    };
    let id = {
        let store = state.store()?;
        // The row the environment made, if that is what is here, gives way:
        // an account set up in the app is the account.
        if let Ok(Some(first)) = store.first_account()
            && store.account_servers(first).ok().flatten().is_none()
            && imap_config_from_env().is_none()
        {
            let _ = store.remove_account(first);
        }
        store
            .add_account(kind, &setup.email, "", &servers)
            .map_err(|e| e.to_string())?
    };
    // Keychain second, so a keychain refusal does not leave a row with no
    // way to sign in. If it fails, the row goes too.
    // Any item already under this id is stale — a removed account whose
    // keychain item outlived its row — and gives way, or an account removed
    // and added again could never sign in: `set_password` refuses to
    // overwrite on macOS.
    if let Err(e) = keychain_entry(id).and_then(|k| {
        let _ = k.delete_credential();
        k.set_password(&setup.password)
            .map_err(|e| format!("keychain: {e}"))
    }) {
        if let Ok(store) = state.store.lock() {
            let _ = store.remove_account(id);
        }
        return Err(e);
    }
    remember_password(id, &setup.password);
    // Syncing starts now, not at the next launch: step 3 of onboarding is
    // "Getting your mail", and it is watching.
    if let Some(cfg) = imap_config(&state, id) {
        spawn_real_sync(Arc::clone(&state), id, cfg);
    }
    Ok(id)
}

/// Makes an account the one the window shows. Nothing about syncing changes:
/// every account is already being kept up to date; this is which one is read.
#[tauri::command]
pub fn set_active_account(account_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    note_ui_touch(&state);
    let store = state.store()?;
    if !store
        .account_ids()
        .map_err(|e| e.to_string())?
        .contains(&account_id)
    {
        return Err("no such account".into());
    }
    store
        .set_active_account(account_id)
        .map_err(|e| e.to_string())
}

/// Removes an account, its mail and its password.
#[tauri::command]
pub fn remove_account(account_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    // Workers first. Left running, the account's drain, send and sync loops
    // kept its server and its queue; and since ids are reused, an account
    // added afterwards inherited them — its triage delivered to the old
    // server, its sends made twice.
    if state.stop_workers(account_id) {
        crate::diag::log_sync(&format!("account {account_id}: workers told to stop"));
    }
    if let Ok(k) = keychain_entry(account_id) {
        // A missing entry is fine; the point is that none remains.
        let _ = k.delete_credential();
    }
    let store = state.store()?;
    store.remove_account(account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_accounts(state: State<Arc<AppState>>) -> Result<Vec<AccountSummary>, String> {
    let store = state.store()?;
    store.accounts().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_account_color(
    account_id: i64,
    color: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    store
        .set_account_color(account_id, &color)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_account_archive(
    account_id: i64,
    enabled: bool,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    store
        .set_local_archive(account_id, enabled)
        .map_err(|e| e.to_string())
}
