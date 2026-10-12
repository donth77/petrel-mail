//! Accounts: discovering a server, testing it, adding and removing accounts, and choosing the one on screen.

use crate::config::{
    forget_password, imap_config, imap_config_from_env, keychain_entry, remember_password,
};
use crate::diag::{Server, friendly_error_for};
use crate::signin::SignIn;
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
#[tauri::command(async)]
pub fn guess_servers(
    address: String,
) -> Option<(petrel_autoconfig::Server, petrel_autoconfig::Server)> {
    petrel_autoconfig::guess(&address)
}

#[derive(Clone, serde::Deserialize)]
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
    check_setup(&setup, which.as_deref())
        .await
        .map_err(|f| f.for_a_person())
}

/// Which half of a connection test failed, and the server's own words.
struct SetupFailure {
    half: &'static str,
    /// Which server that is, so the advice names it.
    server: Server,
    host: String,
    raw: String,
}

impl SetupFailure {
    /// Which server, then what to do about it: the same plain advice the
    /// banner gives. Onboarding showed the reason as it came, and a wrong
    /// password read "sign-in refused: code: None, info: Some(…)".
    fn for_a_person(&self) -> String {
        format!(
            "{} — {}",
            self.half,
            friendly_error_for(self.server, &self.host, &self.raw)
        )
    }
}

async fn check_setup(setup: &AccountSetup, which: Option<&str>) -> Result<(), SetupFailure> {
    let do_imap = which != Some("smtp");
    let do_smtp = which != Some("imap");
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
            .map_err(|e| SetupFailure {
                half: crate::diag::advice("advice-half-imap"),
                server: Server::Incoming,
                host: setup.imap_host.clone(),
                raw: e.to_string(),
            })?;
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
            .map_err(|e| SetupFailure {
                half: crate::diag::advice("advice-half-smtp"),
                server: Server::Outgoing,
                host: setup.smtp_host.clone(),
                raw: e.to_string(),
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod setup_wording_tests {
    use super::{AccountSetup, test_account_inner};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// docs/25 #102. Onboarding showed a failed test in the library's words,
    /// a wrong password as "sign-in refused: code: None, info: Some(…)". It
    /// now says what to do, as Sign in again does. Here the port speaks IMAP
    /// in the clear where the form asked for TLS, as 143 does, so the
    /// handshake reads the greeting as a broken TLS record.
    #[test]
    fn a_failed_test_says_what_to_do_rather_than_what_the_library_saw() {
        let said = tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let _ = sock.write_all(b"* OK IMAP4rev1 ready\r\n").await;
                    let _ = sock.read(&mut [0u8; 1024]).await;
                }
            });
            let setup = AccountSetup {
                email: "me@example.com".into(),
                username: "me@example.com".into(),
                password: "pw".into(),
                imap_host: "127.0.0.1".into(),
                imap_port: port,
                smtp_host: "127.0.0.1".into(),
                smtp_port: 587,
                provider: String::new(),
            };
            test_account_inner(setup, Some("imap".into()))
                .await
                .unwrap_err()
        });
        println!("onboarding says: {said}");
        assert!(said.starts_with("Incoming (IMAP) — "), "{said}");
        assert!(
            !said.contains("tls:") && !said.contains("InvalidContentType"),
            "the library's words reached the form: {said}"
        );
    }

    /// Review r2, R2-5. The outgoing half shared the incoming half's advice,
    /// and a refused SMTP port was told to check whether the network blocks
    /// IMAP. Networks that block one usually block sending: port 25 often,
    /// and some hotel and office networks every SMTP port.
    #[test]
    fn a_refused_outgoing_port_is_told_about_sending() {
        let said = tauri::async_runtime::block_on(async {
            // A port nothing listens on: bound, then let go.
            let port = {
                let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
                l.local_addr().unwrap().port()
            };
            let setup = AccountSetup {
                email: "me@example.com".into(),
                username: "me@example.com".into(),
                password: "pw".into(),
                imap_host: "127.0.0.1".into(),
                imap_port: 993,
                smtp_host: "127.0.0.1".into(),
                smtp_port: port,
                provider: String::new(),
            };
            test_account_inner(setup, Some("smtp".into()))
                .await
                .unwrap_err()
        });
        println!("onboarding says: {said}");
        let (half, advice) = said.split_once(" — ").expect("which half, then the advice");
        assert_eq!(half, "Outgoing (SMTP)");
        assert!(
            advice.contains("outgoing mail (SMTP)") && !advice.contains("IMAP"),
            "{said}"
        );
    }
}

/// The servers half of a setup, as the account row stores it.
fn servers_of(setup: &AccountSetup) -> petrel_engine::store::AccountServers {
    petrel_engine::store::AccountServers {
        imap_host: setup.imap_host.trim().to_string(),
        imap_port: setup.imap_port,
        smtp_host: setup.smtp_host.trim().to_string(),
        smtp_port: setup.smtp_port,
        username: setup.username.trim().to_string(),
        provider: setup.provider.clone(),
    }
}

/// Stores the account: servers on the row, password in the keychain, and
/// then starts syncing it. Only ever called after `test_account` passed, so
/// a wrong password never reaches the keychain.
#[tauri::command(async)]
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
    // A clean stop switch. Account ids are reused — the row has no
    // AUTOINCREMENT — so an account added after a removal inherits the id
    // *and* the flipped switch that stopped the old one's workers. Without
    // this the new account's sync would stand down the moment it started.
    state.reset_workers(id);
    // Nor does it inherit the old one's "sign in again".
    state.clear_signin(id);
    // Syncing starts now, not at the next launch: step 3 of onboarding is
    // "Getting your mail", and it is watching.
    if let Some(cfg) = imap_config(&state, id) {
        spawn_real_sync(Arc::clone(&state), id, cfg);
    }
    Ok(id)
}

/// Makes an account the one the window shows. Nothing about syncing changes:
/// every account is already being kept up to date; this is which one is read.
#[tauri::command(async)]
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
///
/// On the account's turn, as Sign in again is: a removal that landed while a
/// new password was being tested left a keychain item, a cached password and
/// a running sync for an id with no row behind it.
#[tauri::command]
pub async fn remove_account(
    account_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let turn = state.account_turn(account_id);
    let _turn = turn.lock().await;
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
    forget_account(&state, account_id)
}

/// The row and everything the session remembers about the account.
fn forget_account(state: &AppState, account_id: i64) -> Result<(), String> {
    // Nor in memory: the next account given this id must not sign in to its
    // own server with this one's password, or start from what this one's
    // server could do.
    forget_password(account_id);
    state.clear_signin(account_id);
    state.forget_server(account_id);
    let _ = state.take_held(account_id);
    let store = state.store()?;
    store.remove_account(account_id).map_err(|e| e.to_string())
}

/// An account as the window lists it: the store's summary, and whether it
/// needs its password entered again.
#[derive(serde::Serialize)]
pub(crate) struct AccountView {
    #[serde(flatten)]
    summary: AccountSummary,
    signin: Option<SignIn>,
}

#[tauri::command(async)]
pub fn list_accounts(state: State<Arc<AppState>>) -> Result<Vec<AccountView>, String> {
    let store = state.store()?;
    Ok(store
        .accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|summary| AccountView {
            signin: state.signin(summary.id),
            summary,
        })
        .collect())
}

/// Where an account signs in, to start the "Sign in again" form from.
/// Never its password.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct AccountForm {
    email: String,
    username: String,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
    provider: String,
}

#[tauri::command(async)]
pub fn account_form(account_id: i64, state: State<Arc<AppState>>) -> Result<AccountForm, String> {
    let store = state.store()?;
    account_form_from(&store, account_id)
}

fn account_form_from(
    store: &petrel_engine::store::Store,
    account_id: i64,
) -> Result<AccountForm, String> {
    let email = store
        .accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|a| a.id == account_id)
        .map(|a| a.email)
        .ok_or_else(|| "no such account".to_string())?;
    let servers = store
        .account_servers(account_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    Ok(AccountForm {
        username: if servers.username.is_empty() {
            email.clone()
        } else {
            servers.username
        },
        email,
        imap_host: servers.imap_host,
        imap_port: servers.imap_port,
        smtp_host: servers.smtp_host,
        smtp_port: servers.smtp_port,
        provider: servers.provider,
    })
}

/// Signs an account in again: a new password, and, if they changed, its
/// servers, username or address.
///
/// The way back for an account whose password changed, as Thunderbird and
/// Apple Mail ask for it. There used to be none: the only way out was
/// Remove account, which deletes mail that exists nowhere else.
///
/// Tested first, the way onboarding tests a new account, so a password the
/// server refuses changes nothing stored. Then the keychain, then the row,
/// then a fresh first pass: the account's workers hold the old password in
/// their configuration, so they stand down and start again with this one.
#[tauri::command]
pub async fn update_account(
    account_id: i64,
    setup: AccountSetup,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let state = Arc::clone(&state);
    let check = setup.clone();
    let test = async move {
        // Spawned, as `test_account` is, and for the same reason.
        tauri::async_runtime::spawn(async move {
            check_setup(&check, None)
                .await
                .map_err(|f| f.for_a_person())
        })
        .await
        .map_err(|e| format!("test task: {e}"))?
    };
    sign_in_again(
        &state,
        account_id,
        &setup,
        test,
        |id, pass| {
            let entry = keychain_entry(id)?;
            // set_password refuses to overwrite on macOS; clear first.
            let _ = entry.delete_credential();
            entry
                .set_password(pass)
                .map_err(|e| format!("keychain: {e}"))
        },
        imap_config,
    )
    .await?;
    crate::diag::log_sync(&format!("account {account_id}: signed in again"));
    Ok(())
}

/// The order `update_account` keeps, with the server test, the keychain and
/// the new configuration passed in so it can be checked without any of them.
///
/// On the account's turn: two of these at once, or one beside a removal,
/// could leave the keychain, the row and the cache disagreeing, or start two
/// sync loops for one account.
async fn sign_in_again(
    state: &Arc<AppState>,
    account: i64,
    setup: &AccountSetup,
    test: impl std::future::Future<Output = Result<(), String>>,
    write_secret: impl FnOnce(i64, &str) -> Result<(), String>,
    config_for: impl FnOnce(&AppState, i64) -> Option<ImapConfig>,
) -> Result<(), String> {
    let turn = state.account_turn(account);
    let _turn = turn.lock().await;
    still_here(state, account)?;
    test.await?;
    // Looked at again right before anything is written: removed while the
    // server was asked, the account gets no keychain item or password back.
    still_here(state, account)?;
    apply_new_credentials(state, account, setup, write_secret)?;
    let cfg = config_for(state, account);
    restart_with(state, account, cfg);
    Ok(())
}

fn still_here(state: &AppState, account: i64) -> Result<(), String> {
    let store = state.store()?;
    if store
        .account_ids()
        .map_err(|e| e.to_string())?
        .contains(&account)
    {
        Ok(())
    } else {
        Err("no such account".into())
    }
}

/// Stores credentials that passed their test. The keychain first: a refusal
/// there is reported before the row or the cache changes.
///
/// New servers make the account an ordinary IMAP account again until its
/// probe says otherwise. The kind was only ever set to Gmail, so an account
/// moved off Gmail kept Gmail's labels policy, and its Archive was never
/// synced.
fn apply_new_credentials(
    state: &AppState,
    account: i64,
    setup: &AccountSetup,
    write_secret: impl FnOnce(i64, &str) -> Result<(), String>,
) -> Result<(), String> {
    write_secret(account, &setup.password)?;
    {
        let store = state.store()?;
        let servers = servers_of(setup);
        let moved = store
            .account_servers(account)
            .map_err(|e| e.to_string())?
            .is_some_and(|old| !old.imap_host.eq_ignore_ascii_case(&servers.imap_host));
        store
            .set_account_servers(account, &servers)
            .map_err(|e| e.to_string())?;
        if moved {
            store
                .set_account_kind(account, "imap")
                .map_err(|e| e.to_string())?;
        }
        let email = setup.email.trim();
        if !email.is_empty() {
            store
                .set_account_email(account, email)
                .map_err(|e| e.to_string())?;
        }
    }
    remember_password(account, &setup.password);
    Ok(())
}

/// Starts an account over, with `cfg`: none starts nothing.
///
/// The old workers stop first, and only then is the slate cleared: the
/// sign-in state, the banner, and what the session knew about the server.
/// A worker of the old run that meets a refusal after this reads its switch
/// as stopped and writes nothing (`AppState::refused_by`), so the new
/// workers never stand down on the old password's account of things.
fn restart_with(state: &Arc<AppState>, account: i64, cfg: Option<ImapConfig>) {
    state.stop_workers(account);
    state.reset_workers(account);
    state.forget_server(account);
    state.clear_signin(account);
    // The banner was most likely this account's refusal. The fresh pass
    // puts back anything still wrong, here or on another account.
    *state.sync_error.lock().unwrap_or_else(|p| p.into_inner()) = None;
    if let Some(cfg) = cfg {
        spawn_real_sync(Arc::clone(state), account, cfg);
    }
}

#[tauri::command(async)]
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

#[tauri::command(async)]
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

#[cfg(test)]
mod sign_in_again_tests {
    use super::*;
    use crate::config::cached_password;
    use crate::state::test_state;
    use petrel_engine::store::AccountServers;
    use std::cell::{Cell, RefCell};

    /// The password cache is the process's, and these tests share account
    /// id 1 in their separate stores: one at a time.
    static CACHE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn setup(password: &str) -> AccountSetup {
        AccountSetup {
            email: "me@new.example".into(),
            username: "me-new".into(),
            password: password.into(),
            imap_host: "imap.new.example".into(),
            imap_port: 993,
            smtp_host: "smtp.new.example".into(),
            smtp_port: 465,
            provider: "New Mail".into(),
        }
    }

    fn old_servers() -> AccountServers {
        AccountServers {
            imap_host: "imap.old.example".into(),
            imap_port: 993,
            smtp_host: "smtp.old.example".into(),
            smtp_port: 465,
            username: "me-old".into(),
            provider: String::new(),
        }
    }

    fn email_of(state: &AppState, id: i64) -> String {
        state
            .store()
            .unwrap()
            .accounts()
            .unwrap()
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
            .email
    }

    /// The point of testing first: a password the server refuses changes
    /// nothing — not the keychain, the row, the cache or the sign-in state.
    #[test]
    fn a_password_the_server_refuses_changes_nothing() {
        let _turn = crate::config::cache_turn();
        let _one = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = state.account_id;
        state
            .store()
            .unwrap()
            .set_account_servers(id, &old_servers())
            .unwrap();
        state.set_signin(id, SignIn::Refused);
        remember_password(id, "old-pass");
        let wrote = Cell::new(false);
        let restarted = Cell::new(false);
        let result = tauri::async_runtime::block_on(sign_in_again(
            &state,
            id,
            &setup("still-wrong"),
            async { Err("Incoming (IMAP) — Sign-in was refused.".to_string()) },
            |_, _| {
                wrote.set(true);
                Ok(())
            },
            |_, _| {
                restarted.set(true);
                None
            },
        ));
        assert_eq!(result, Err("Incoming (IMAP) — Sign-in was refused.".into()));
        assert!(!wrote.get(), "nothing reached the keychain");
        assert!(!restarted.get(), "the old workers were left alone");
        assert_eq!(
            state.store().unwrap().account_servers(id).unwrap(),
            Some(old_servers())
        );
        assert_eq!(email_of(&state, id), "test@example.com");
        assert_eq!(cached_password(id).as_deref(), Some("old-pass"));
        assert_eq!(state.signin(id), Some(SignIn::Refused));
    }

    #[test]
    fn a_password_that_works_is_stored_and_the_account_starts_over() {
        let _turn = crate::config::cache_turn();
        let _one = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = state.account_id;
        state
            .store()
            .unwrap()
            .set_account_servers(id, &old_servers())
            .unwrap();
        state.set_signin(id, SignIn::Refused);
        remember_password(id, "old-pass");
        let wrote = RefCell::new(None);
        let restarted = Cell::new(None);
        let result = tauri::async_runtime::block_on(sign_in_again(
            &state,
            id,
            &setup("new-pass"),
            async { Ok(()) },
            |which, pass| {
                *wrote.borrow_mut() = Some((which, pass.to_string()));
                Ok(())
            },
            |_, which| {
                restarted.set(Some(which));
                None
            },
        ));
        assert_eq!(result, Ok(()));
        assert_eq!(*wrote.borrow(), Some((id, "new-pass".to_string())));
        assert_eq!(
            state.store().unwrap().account_servers(id).unwrap(),
            Some(servers_of(&setup("new-pass")))
        );
        assert_eq!(email_of(&state, id), "me@new.example");
        assert_eq!(cached_password(id).as_deref(), Some("new-pass"));
        assert_eq!(state.signin(id), None, "signed in");
        assert_eq!(restarted.get(), Some(id), "its workers start over");
    }

    #[test]
    fn a_keychain_that_refuses_the_write_leaves_the_account_as_it_was() {
        let _turn = crate::config::cache_turn();
        let _one = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = state.account_id;
        state
            .store()
            .unwrap()
            .set_account_servers(id, &old_servers())
            .unwrap();
        state.set_signin(id, SignIn::Missing);
        remember_password(id, "old-pass");
        let restarted = Cell::new(false);
        let result = tauri::async_runtime::block_on(sign_in_again(
            &state,
            id,
            &setup("new-pass"),
            async { Ok(()) },
            |_, _| Err("keychain: denied".to_string()),
            |_, _| {
                restarted.set(true);
                None
            },
        ));
        assert_eq!(result, Err("keychain: denied".into()));
        assert_eq!(
            state.store().unwrap().account_servers(id).unwrap(),
            Some(old_servers())
        );
        assert_eq!(cached_password(id).as_deref(), Some("old-pass"));
        assert_eq!(state.signin(id), Some(SignIn::Missing));
        assert!(!restarted.get());
    }

    #[test]
    fn an_account_that_is_not_there_is_refused_before_any_test() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let tested = Cell::new(false);
        let result = tauri::async_runtime::block_on(sign_in_again(
            &state,
            4242,
            &setup("p"),
            async {
                tested.set(true);
                Ok(())
            },
            |_, _| Ok(()),
            |_, _| None,
        ));
        assert_eq!(result, Err("no such account".into()));
        assert!(!tested.get());
    }

    /// The review's finding 6. Removed while its new password was being
    /// tested, the account got its keychain item, its cached password and a
    /// fresh stop switch back, for an id with no row.
    #[test]
    fn an_account_removed_during_the_test_gets_nothing_written() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = crate::signin::test_support::unkeyed_account(&state);
        let wrote = Cell::new(false);
        let result = tauri::async_runtime::block_on(sign_in_again(
            &state,
            id,
            &setup("new-pass"),
            async {
                // The row goes while the server is asked.
                state.store().unwrap().remove_account(id).unwrap();
                Ok(())
            },
            |_, _| {
                wrote.set(true);
                Ok(())
            },
            |_, _| None,
        ));
        assert_eq!(result, Err("no such account".into()));
        assert!(!wrote.get(), "no keychain item for an id with no row");
        assert_eq!(cached_password(id), None);
    }

    /// Two at once take turns: the second is not even tested until the first
    /// has written everything and started its account over.
    #[test]
    fn two_sign_ins_of_one_account_take_turns() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = crate::signin::test_support::unkeyed_account(&state);
        let order = std::sync::Mutex::new(Vec::new());
        let (one, two) = (setup("first"), setup("second"));
        tauri::async_runtime::block_on(async {
            let (release, held) = tokio::sync::oneshot::channel::<()>();
            let first = sign_in_again(
                &state,
                id,
                &one,
                async {
                    order.lock().unwrap().push("first tested");
                    let _ = held.await;
                    Ok(())
                },
                |_, _| {
                    order.lock().unwrap().push("first written");
                    Ok(())
                },
                |_, _| None,
            );
            let second = sign_in_again(
                &state,
                id,
                &two,
                async {
                    order.lock().unwrap().push("second tested");
                    Ok(())
                },
                |_, _| {
                    order.lock().unwrap().push("second written");
                    Ok(())
                },
                |_, _| None,
            );
            let releaser = async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                order.lock().unwrap().push("released");
                let _ = release.send(());
            };
            let (a, b, ()) = tokio::join!(first, second, releaser);
            assert_eq!((a, b), (Ok(()), Ok(())));
        });
        crate::config::forget_password(id);
        assert_eq!(
            *order.lock().unwrap(),
            vec![
                "first tested",
                "released",
                "first written",
                "second tested",
                "second written"
            ]
        );
    }

    /// The review's finding 5. Moved to another server, the account is plain
    /// IMAP until its probe says otherwise: kept as Gmail, it kept Gmail's
    /// labels policy, and its Archive was never synced.
    #[test]
    fn new_servers_make_the_account_plain_imap_until_its_probe_says_otherwise() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = crate::signin::test_support::unkeyed_account(&state);
        {
            let store = state.store().unwrap();
            store
                .set_account_servers(
                    id,
                    &AccountServers {
                        imap_host: "imap.gmail.com".into(),
                        ..old_servers()
                    },
                )
                .unwrap();
            store.set_account_kind(id, "gmail").unwrap();
        }
        let kind = |state: &AppState| {
            state
                .store()
                .unwrap()
                .accounts()
                .unwrap()
                .into_iter()
                .find(|a| a.id == id)
                .unwrap()
                .kind
        };
        // The same server: the kind stands.
        let same = AccountSetup {
            imap_host: "IMAP.gmail.com".into(),
            ..setup("p")
        };
        apply_new_credentials(&state, id, &same, |_, _| Ok(())).unwrap();
        assert_eq!(kind(&state), "gmail");
        apply_new_credentials(&state, id, &setup("p"), |_, _| Ok(())).unwrap();
        crate::config::forget_password(id);
        assert_eq!(kind(&state), "imap");
    }

    /// Restarting is what puts the new password to use: the workers took the
    /// old one with them when they started. And the old run's notion of the
    /// server goes with them (the review's finding 5): kept, a new first pass
    /// that found no network never started its IDLE watchers.
    ///
    /// No configuration is handed in, so nothing is started, and nothing can
    /// reach the developer's `PETREL_IMAP_*` variables (the review's finding
    /// 7: `restart_account` fell back to them).
    #[test]
    fn starting_over_stops_the_old_workers_and_clears_the_slate() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = state.account_id;
        let old = state.stop_signal(id);
        *state.sync_error.lock().unwrap() = Some("Sign-in was refused.".into());
        state.set_signin(id, SignIn::Refused);
        state.set_caps(
            id,
            crate::state::ServerCaps {
                has_idle: true,
                known: true,
                ..Default::default()
            },
        );
        state.mark_surveyed(id);
        restart_with(&state, id, None);
        assert!(*old.borrow(), "the old workers read stopped");
        assert!(!*state.stop_signal(id).borrow(), "the new ones start clean");
        assert_eq!(*state.sync_error.lock().unwrap(), None);
        assert_eq!(state.signin(id), None);
        assert!(!state.caps(id).known, "what the old run learned is gone");
        assert!(!state.surveyed(id));
        // A refusal the old run meets afterwards is not written over the
        // fresh start.
        assert!(!state.refused_by(id, &old));
        assert_eq!(state.signin(id), None);
    }

    /// Ids are reused. A removed account's password, kept in memory, went to
    /// the server of whatever account took its id next (docs/25 #126).
    #[test]
    fn a_removed_account_leaves_no_password_or_sign_in_state_behind() {
        let _turn = crate::config::cache_turn();
        let _one = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = state.account_id;
        remember_password(id, "removed-accounts-pass");
        state.set_signin(id, SignIn::Refused);
        state.set_caps(
            id,
            crate::state::ServerCaps {
                is_gmail: true,
                known: true,
                ..Default::default()
            },
        );
        state.hold_draft(id, 7);
        forget_account(&state, id).unwrap();
        assert_eq!(cached_password(id), None);
        assert_eq!(state.signin(id), None);
        assert!(
            !state.caps(id).known,
            "the next account given the id starts afresh"
        );
        assert_eq!(state.take_held(id), crate::signin::Held::default());
        assert!(!state.store().unwrap().account_ids().unwrap().contains(&id));
    }

    #[test]
    fn the_form_starts_from_where_the_account_signs_in_and_never_its_password() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let id = state.account_id;
        let store = state.store().unwrap();
        store.set_account_servers(id, &old_servers()).unwrap();
        let form = account_form_from(&store, id).unwrap();
        assert_eq!(
            form,
            AccountForm {
                email: "test@example.com".into(),
                username: "me-old".into(),
                imap_host: "imap.old.example".into(),
                imap_port: 993,
                smtp_host: "smtp.old.example".into(),
                smtp_port: 465,
                provider: String::new(),
            }
        );
        // No username stored: the address is the username, as onboarding assumes.
        store
            .set_account_servers(
                id,
                &AccountServers {
                    username: String::new(),
                    ..old_servers()
                },
            )
            .unwrap();
        assert_eq!(
            account_form_from(&store, id).unwrap().username,
            "test@example.com"
        );
        assert!(account_form_from(&store, 4242).is_err());
    }
}

#[cfg(all(test, feature = "dev-plaintext-imap"))]
mod signin_race_tests {
    //! Sign in again against a server that counts every LOGIN.
    use super::*;
    use crate::scripted_imap::{Srv, serve};
    use crate::signin::refused_password_tests::{fast_clocks, plain};
    use crate::state::test_state;
    use std::time::{Duration, Instant};

    fn two_folders(state: &AppState) {
        state
            .store
            .lock()
            .unwrap()
            .sync_folders(
                state.account_id,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Sent".into(), Some("sent".into())),
                ],
            )
            .unwrap();
    }

    /// The review's probe E. The old loop's hourly question is answered after
    /// the new password has been saved.
    ///
    /// The account is refused and parked; its hour comes up and the old loop
    /// asks once with the old password, which the server takes two seconds
    /// to refuse (Dovecot's default delay). In those two seconds the person
    /// saves a new password that works. The old loop's refusal used to land
    /// afterwards and mark the account refused again, with the banner, and
    /// the new loop stood down for an hour on a password that worked.
    #[test]
    fn a_late_refusal_from_the_stopped_loop_does_not_outlive_the_new_password() {
        let _turn = crate::config::cache_turn();
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        two_folders(&state);
        let srv = Srv::new(
            "UIDPLUS",
            &[("INBOX", ""), ("Sent", "\\Sent")],
            &["new-pass"],
        );
        *srv.login_delay.lock().unwrap() = Duration::from_secs(2);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            spawn_real_sync(Arc::clone(&state), account, plain(port, "old-pass"));
            let mut parked = false;
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if state.signin(account) == Some(SignIn::Refused) {
                    parked = true;
                    break;
                }
            }
            assert!(parked, "the old loop stood down");
            tokio::time::sleep(Duration::from_secs(1)).await;

            // An hour passes, and the old loop asks once more.
            let asked_from = Instant::now();
            state.signin.lock().unwrap().insert(
                account,
                (SignIn::Refused, crate::state::now_ms() - 2 * 3_600_000),
            );
            state.signin_changed.send_modify(|n| *n += 1);
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if srv.logins_with("old-pass", asked_from) > 0 {
                    break;
                }
            }
            assert_eq!(
                srv.logins_with("old-pass", asked_from),
                1,
                "the hourly question"
            );

            // While the server is still thinking it over, Sign in again.
            let setup = AccountSetup {
                email: "test@example.com".into(),
                username: "u".into(),
                password: "new-pass".into(),
                imap_host: "127.0.0.1".into(),
                imap_port: port,
                smtp_host: "127.0.0.1".into(),
                smtp_port: port,
                provider: String::new(),
            };
            let signed_in = sign_in_again(
                &state,
                account,
                &setup,
                async { Ok(()) },
                |_, _| Ok(()),
                |_, _| Some(plain(port, "new-pass")),
            )
            .await;
            assert_eq!(signed_in, Ok(()));
            let saved_at = Instant::now();
            // Long past the old refusal's landing (two seconds after it was
            // asked) and the new first pass: a refusal written over the new
            // password would stand for an hour, so it would still be here.
            tokio::time::sleep(Duration::from_secs(12)).await;
            let why = state.signin(account);
            let banner = state.sync_error.lock().unwrap().clone();
            let new_logins = srv.logins_with("new-pass", saved_at);
            state.stop_workers(account);
            crate::config::forget_password(account);
            assert_eq!(
                why,
                None,
                "signed in, as the dialog said: {}",
                srv.timeline()
            );
            assert_eq!(banner, None);
            assert!(
                new_logins >= 2,
                "the new first pass signed in: {}",
                srv.timeline()
            );
        });
    }

    /// The review's probe H, through the restart Sign in again uses. The
    /// launch probe finds no network, and a later sweep learns the server can
    /// IDLE: a fresh account starts IDLE then, and so must one started over,
    /// whatever its previous run had learned.
    #[test]
    fn a_restarted_account_starts_idle_once_a_sweep_learns_it_can() {
        let _turn = crate::config::cache_turn();
        fast_clocks();
        let run = |stale: bool| {
            let dir = tempfile::tempdir().unwrap();
            let state = test_state(dir.path());
            let account = state.account_id;
            two_folders(&state);
            if stale {
                // What the account's previous run left behind.
                state.set_caps(
                    account,
                    crate::state::ServerCaps {
                        has_idle: true,
                        has_move: true,
                        has_uidplus: true,
                        known: true,
                        ..Default::default()
                    },
                );
                state.mark_surveyed(account);
            }
            let srv = Srv::new(
                "IDLE MOVE UIDPLUS",
                &[("INBOX", ""), ("Sent", "\\Sent")],
                &["p"],
            );
            // The first connection, the launch probe, finds no network.
            srv.drop_first.store(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                let port = serve(Arc::clone(&srv)).await;
                restart_with(&state, account, Some(plain(port, "p")));
                // Past the first 15-second poll, whose sweep surveys again.
                tokio::time::sleep(Duration::from_secs(19)).await;
                state.stop_workers(account);
                srv.seen
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|l| l.to_ascii_uppercase().ends_with(" IDLE"))
                    .count()
            }
        };
        let (fresh, restarted) =
            tauri::async_runtime::block_on(async { tokio::join!(run(false), run(true)) });
        assert!(
            fresh > 0,
            "control: a fresh launch starts IDLE once a sweep learns it"
        );
        assert!(restarted > 0, "a restarted account starts IDLE too");
    }

    /// The second review's R2-A. A drain pass in flight when Sign in again
    /// lands.
    ///
    /// The password changes at the provider; Dovecot keeps the open sessions,
    /// so nothing has stood the account down yet. One archive is made, and
    /// the drain worker signs in with the old password; the server takes two
    /// seconds to refuse it (Dovecot's auth_failure_delay). In those two
    /// seconds the new password is saved. The drain's refusal used to land
    /// afterwards, unfenced by its stopped switch, and the new run stood down
    /// for an hour on a password that worked, with the archive held behind it.
    #[test]
    fn a_drain_refusal_in_flight_does_not_undo_sign_in_again() {
        use crate::scripted_imap::raw;
        use petrel_engine::actions::{ActionKind, PlacementPolicy};
        fast_clocks();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        // Sign in again writes servers for it: an id no keychain item has.
        let account = crate::signin::test_support::unkeyed_account(&state);
        let srv = Srv::new(
            "IDLE MOVE UIDPLUS",
            &[("INBOX", ""), ("Archive", "\\Archive")],
            &["old-pass"],
        );
        *srv.refuse_delay.lock().unwrap() = Duration::from_secs(2);
        {
            let mut store = state.store.lock().unwrap();
            store
                .sync_folders(
                    account,
                    &[
                        ("INBOX".into(), Some("inbox".into())),
                        ("Archive".into(), Some("archive".into())),
                    ],
                )
                .unwrap();
            let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
            store.set_backfill_floor(inbox, 1).unwrap();
            srv.put("INBOX", 1, raw(1));
            store
                .ingest_raw(&state.blobs, account, Some(inbox), Some(1), &raw(1))
                .unwrap();
        }
        let _turn = crate::config::cache_turn();
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            spawn_real_sync(Arc::clone(&state), account, plain(port, "old-pass"));
            tokio::time::sleep(Duration::from_secs(4)).await;
            assert!(!state.is_seeding(account), "the first pass is over");
            assert_eq!(state.signin(account), None);
            // The password changes at the provider; open sessions stay up.
            {
                let mut accept = srv.accept.lock().unwrap();
                accept.clear();
                accept.insert("new-pass".into());
            }
            // One archive.
            let archived = Instant::now();
            {
                let store = state.store.lock().unwrap();
                let inbox = store.folder_for_role(account, "inbox").unwrap().unwrap();
                let id = store.message_id_at(inbox, 1).unwrap().unwrap();
                let thread = store.thread_of(id).unwrap().unwrap_or(-id);
                store
                    .apply_thread_action(
                        account,
                        thread,
                        ActionKind::Archive,
                        None,
                        PlacementPolicy::Exclusive,
                    )
                    .unwrap();
            }
            state.nudge_drain(account);
            // The drain worker asks with the old password ...
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if srv.logins_with("old-pass", archived) > 0 {
                    break;
                }
            }
            assert_eq!(
                srv.logins_with("old-pass", archived),
                1,
                "the drain asked: {}",
                srv.timeline()
            );
            // ... and while the server thinks it over, the new password is saved.
            let setup = AccountSetup {
                email: "test@example.com".into(),
                username: "u".into(),
                password: "new-pass".into(),
                imap_host: "127.0.0.1".into(),
                imap_port: port,
                smtp_host: "127.0.0.1".into(),
                smtp_port: port,
                provider: String::new(),
            };
            let signed_in = sign_in_again(
                &state,
                account,
                &setup,
                async { Ok(()) },
                |_, _| Ok(()),
                |_, _| Some(plain(port, "new-pass")),
            )
            .await;
            assert_eq!(signed_in, Ok(()));
            // Past the old refusal's landing, and until the archive has gone,
            // which the new run's first pass or its 30-second sweep delivers.
            let mut delivered = false;
            for _ in 0..(36 * 4) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let queued = state
                    .store
                    .lock()
                    .unwrap()
                    .pending_actions(account)
                    .unwrap()
                    .len();
                if queued == 0 && archived.elapsed() > Duration::from_secs(5) {
                    delivered = true;
                    break;
                }
            }
            // And still signed in once the refusal has certainly landed.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let why = state.signin(account);
            let timeline = srv.timeline();
            state.stop_workers(account);
            crate::config::forget_password(account);
            assert_eq!(
                why, None,
                "signed in with a password that works, but stood down: {timeline}"
            );
            assert!(
                delivered,
                "the archive waited behind a stale refusal: {timeline}"
            );
        });
    }
}
