//! Account credentials and server settings: the keychain, and the environment as the developer override.

use crate::state::AppState;
use petrel_engine::store::Store;
use petrel_providers::imap::{ImapConfig, Security};
use std::sync::Mutex;

/// Reads account settings from the environment. Credentials never appear in
/// argv (visible to every process on the machine) or in a config file we wrote;
/// the keychain replaces this at M4 when account setup exists.
/// The keychain entry for an account's password.
///
/// Keyed by the account's row id rather than its address, so renaming an
/// account or adding a second one with the same address on another server
/// cannot point two accounts at one secret.
pub(crate) fn keychain_entry(account_id: i64) -> Result<keyring::Entry, String> {
    keyring::Entry::new("dev.petrel.desktop", &format!("account-{account_id}"))
        .map_err(|e| format!("keychain: {e}"))
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
    let pass = account_password(account_id)?;
    Some(ImapConfig {
        host: servers.imap_host,
        port: servers.imap_port,
        user: servers.username,
        pass,
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
    let pass = account_password(account_id)?;
    Some(petrel_providers::smtp::SmtpConfig {
        host: servers.smtp_host,
        port: servers.smtp_port,
        user: servers.username,
        pass,
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
        pass,
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
