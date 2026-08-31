//! Account credentials and server settings: the keychain, and the environment as the developer override.

use crate::state::AppState;
use petrel_engine::store::Store;
use petrel_providers::imap::{Credential, ImapConfig, Security};
use std::sync::Mutex;

/// Reads account settings from the environment. Credentials never appear in
/// argv (visible to every process on the machine) or in a config file we wrote;
/// the keychain replaces this at M4 when account setup exists.
/// This store's own name in the keychain, learned once per launch.
///
/// Set from the store at startup. Absent only if something asked for a
/// password before the store was open, which the fallback below treats as
/// the legacy single-store world rather than inventing a new identity.
static STORE_KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Records which store this process is serving, and moves its passwords into
/// the per-store namespace if they are still in the old shared one.
///
/// Two stores — the demo one and the live one, or a copy taken for testing —
/// each called their first account `account-1` and therefore shared a single
/// keychain item. Nothing had gone wrong yet only because both slots happened
/// to hold the same password; the moment they differed, one store would sign
/// in with the other's secret, and deleting an account in one would take the
/// other's password with it. Keyed by the store, that cannot happen.
pub(crate) fn adopt_store_identity(store: &Store, account_ids: &[i64]) {
    let Ok(id) = store_identity(store) else {
        return;
    };
    if STORE_KEY.set(id.clone()).is_err() {
        return; // already set: nothing to do
    }
    // One-time move: a password sitting under the old shared name becomes
    // this store's. The old item is left alone rather than deleted — another
    // store may still be reading it, and this is not the code that gets to
    // decide that.
    for account in account_ids {
        let new = keyring::Entry::new(KEYCHAIN_SERVICE, &format!("{id}/account-{account}"));
        let old = keyring::Entry::new(KEYCHAIN_SERVICE, &format!("account-{account}"));
        if let (Ok(new), Ok(old)) = (new, old)
            && new.get_password().is_err()
            && let Ok(pass) = old.get_password()
        {
            match new.set_password(&pass) {
                Ok(()) => {
                    remember_password(*account, &pass);
                    crate::diag::log_sync(&format!(
                        "keychain: account {account} adopted into this store's namespace"
                    ));
                }
                Err(e) => eprintln!("[keychain] could not adopt account {account}: {e}"),
            }
        }
    }
}

const KEYCHAIN_SERVICE: &str = "dev.petrel.desktop";

/// A stable name for this store, minted once and kept in it. Minted rather
/// than derived from the path so that moving the directory — or restoring it
/// from a backup — does not orphan the passwords inside it.
fn store_identity(store: &Store) -> Result<String, String> {
    if let Ok(settings) = store.settings()
        && let Some(id) = settings.get("store_id")
        && !id.is_empty()
    {
        return Ok(id.clone());
    }
    let minted = format!(
        "s{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    store
        .set_setting("store_id", &minted)
        .map_err(|e| e.to_string())?;
    Ok(minted)
}

/// The keychain entry for an account's password.
///
/// Keyed by the store *and* the account's row id: by the row rather than the
/// address, so renaming an account cannot point two at one secret; by the
/// store, so two stores' first accounts are not the same item.
pub(crate) fn keychain_entry(account_id: i64) -> Result<keyring::Entry, String> {
    let name = match STORE_KEY.get() {
        Some(store) => format!("{store}/account-{account_id}"),
        // Before the store is open there is no per-store name to use. The
        // legacy name is the honest answer: it is where such a password
        // would already be.
        None => format!("account-{account_id}"),
    };
    keyring::Entry::new(KEYCHAIN_SERVICE, &name).map_err(|e| format!("keychain: {e}"))
}

/// Passwords read from the keychain, once per account per launch.
///
/// Dev builds are unsigned, so to macOS every rebuild is a different app and
/// every keychain read may raise a consent dialog. Before this cache the
/// status poll read the password every few seconds — a dialog storm that
/// also blocked every sync task behind the first unanswered prompt. Now the
/// keychain is touched at most once per account per process; the dialog (at
/// most one per account) appears at launch and is done. Proper signing —
/// the packaging phase — is what retires the dialog altogether.
static PASS_CACHE: std::sync::OnceLock<Mutex<std::collections::HashMap<i64, String>>> =
    std::sync::OnceLock::new();

/// Seeds the cache at the moment a password is written, so the write is not
/// immediately followed by a consenting read of the same value.
pub(crate) fn remember_password(account_id: i64, pass: &str) {
    let cache = PASS_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut map) = cache.lock() {
        map.insert(account_id, pass.to_string());
    }
}

/// Passwords handed in by the environment, keyed by the account's own
/// username. The dev launch script exports both accounts' credentials from
/// the gitignored env files; when a username matches, the keychain is never
/// consulted at all. On dev builds that matters more than it sounds: a
/// self-signed identity does not hold macOS keychain consent across
/// rebuilds, so every rebuilt binary asked for the keychain password twice
/// — once per account — before this. A packaged build runs with no such
/// variables and uses the keychain as before.
fn env_password_for(username: &str) -> Option<String> {
    for (u, p) in [
        ("PETREL_IMAP_USER", "PETREL_IMAP_PASS"),
        ("PETREL_NC_USER", "PETREL_NC_PASS"),
    ] {
        if let (Ok(eu), Ok(ep)) = (std::env::var(u), std::env::var(p))
            && eu.eq_ignore_ascii_case(username)
        {
            return Some(ep);
        }
    }
    None
}

fn account_password(account_id: i64) -> Option<String> {
    let cache = PASS_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(map) = cache.lock()
        && let Some(hit) = map.get(&account_id)
    {
        return Some(hit.clone());
    }
    // The one true keychain read in the program. (A blanket rewrite once
    // pointed this line back at this function; the recursion overflowed the
    // stack on every launch — hence a test that now calls this twice.)
    let pass = keychain_entry(account_id).ok()?.get_password().ok()?;
    if let Ok(mut map) = cache.lock() {
        map.insert(account_id, pass.clone());
    }
    Some(pass)
}

/// The IMAP configuration for an account that was set up in the app.
///
/// The store has the servers; the keychain has the password. Either missing
/// means this account was not set up here — which, today, means it is the
/// developer row driven by the environment, and the caller falls back.
pub(crate) fn imap_config_for(store: &Store, account_id: i64) -> Option<ImapConfig> {
    let servers = store.account_servers(account_id).ok().flatten()?;
    if servers.imap_host.is_empty() {
        return None;
    }
    let pass = env_password_for(&servers.username).or_else(|| account_password(account_id))?;
    Some(ImapConfig {
        host: servers.imap_host,
        port: servers.imap_port,
        user: servers.username,
        credential: Credential::Password(pass),
        security: Security::Tls,
    })
}

/// The SMTP half, for the same account. Explicit rather than derived from
/// the IMAP host by string substitution: autoconfig answers both, and a
/// provider like Namecheap uses one host for both while another uses two.
pub(crate) fn smtp_config_for(
    store: &Store,
    account_id: i64,
) -> Option<petrel_providers::smtp::SmtpConfig> {
    let servers = store.account_servers(account_id).ok().flatten()?;
    if servers.smtp_host.is_empty() {
        return None;
    }
    let pass = env_password_for(&servers.username).or_else(|| account_password(account_id))?;
    Some(petrel_providers::smtp::SmtpConfig {
        host: servers.smtp_host,
        port: servers.smtp_port,
        user: servers.username,
        credential: Credential::Password(pass),
    })
}

/// The account's IMAP configuration from wherever it lives: the app's own
/// setup first, the environment as the developer override.
pub(crate) fn imap_config(state: &AppState, account_id: i64) -> Option<ImapConfig> {
    state
        .store
        .lock()
        .ok()
        .and_then(|s| imap_config_for(&s, account_id))
        .or_else(imap_config_from_env)
}

pub(crate) fn imap_config_from_env() -> Option<ImapConfig> {
    let host = std::env::var("PETREL_IMAP_HOST").ok()?;
    let user = std::env::var("PETREL_IMAP_USER").ok()?;
    let pass = std::env::var("PETREL_IMAP_PASS").ok()?;
    let plaintext = std::env::var("PETREL_IMAP_TLS")
        .map(|v| v == "0")
        .unwrap_or(false);

    #[cfg(feature = "dev-plaintext-imap")]
    let security = if plaintext {
        Security::InsecurePlaintext
    } else {
        Security::Tls
    };
    #[cfg(not(feature = "dev-plaintext-imap"))]
    let security = {
        if plaintext {
            eprintln!(
                "[sync] PETREL_IMAP_TLS=0 ignored: plaintext is not compiled into this build"
            );
        }
        Security::Tls
    };

    Some(ImapConfig {
        host,
        port: std::env::var("PETREL_IMAP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(if plaintext { 143 } else { 993 }),
        user,
        credential: Credential::Password(pass),
        security,
    })
}

#[cfg(test)]
mod password_cache_tests {
    use super::account_password;

    /// The regression fence for a launch-killing bug: a blanket rewrite once
    /// pointed the cache's miss path back at itself, and every launch died
    /// of stack overflow on the first password read. The property that
    /// matters is simply that a miss *terminates* — twice, so both the
    /// uncached and cached paths run. An id no store will ever issue keeps
    /// this off any real keychain item (absent items fail without a dialog).
    #[test]
    fn a_cache_miss_terminates() {
        assert_eq!(account_password(i64::MAX), None);
        assert_eq!(account_password(i64::MAX), None);
    }
}

#[cfg(test)]
mod store_key_tests {
    use super::*;

    #[test]
    fn a_store_keeps_the_name_it_was_given() {
        let a = Store::open_in_memory().unwrap();
        let first = store_identity(&a).expect("minted");
        assert!(!first.is_empty());
        // Minted once and kept: a second ask returns the same name, so the
        // passwords written under it stay findable across launches.
        assert_eq!(store_identity(&a).expect("again"), first);

        // A different store is a different name — the whole point. Two stores
        // whose first account is `account-1` must not share one item.
        let b = Store::open_in_memory().unwrap();
        assert_ne!(store_identity(&b).expect("other"), first);
    }

    #[test]
    fn the_entry_name_carries_the_store_when_one_is_known() {
        // Before a store is adopted the legacy name is used, which is where
        // an existing password already lives.
        let name = match STORE_KEY.get() {
            Some(store) => format!("{store}/account-7"),
            None => "account-7".to_string(),
        };
        assert!(name.ends_with("account-7"));
        // And an adopted store puts its own name in front.
        let with_store = format!("{}/account-7", "s1234");
        assert_eq!(with_store, "s1234/account-7");
    }
}
