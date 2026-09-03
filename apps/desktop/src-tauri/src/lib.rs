//! Petrel's desktop shell: a thin window over the engine. All real work
//! happens in `petrel-engine`; this crate wires typed IPC and (soon) the
//! `petrel-msg://` custom protocol for sanitized message documents.
//!
//! Two source modes: with `PETREL_IMAP_*` set it syncs a real mailbox through
//! the engine's ingest path; without, it seeds synthetic mail so the UI is
//! exercisable with no account. Both run the same store, index, and queries.
//!
//! What lives where: `state` is what every part shares; `commands` is the IPC
//! surface, one file per area of the UI; `sync` and `send` are the workers
//! that keep the server and the store in step; `config`, `demo` and `diag`
//! are what their names say. This file is the bootstrap — `run()` — and
//! nothing else, so that the shape of the app is visible from its module list.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;
use petrel_providers::imap::ImapConfig;
use tauri::Manager;

mod commands;
mod config;
mod demo;
mod diag;
// Public so the render path can be tested directly. The privacy guarantees
// live in this module, and they are worth asserting on rather than trusting.
pub mod message_view;
mod notify;
mod send;
// The webview isolation matrix: hostile documents served to a real frame so
// the layers can be measured from outside. Useful, and not something a
// shipping binary should be able to serve — a release build has no such
// routes at all.
#[cfg(debug_assertions)]
mod spike_s2;
mod state;
mod sync;

use config::{
    adopt_legacy_keychain_items, adopt_store_identity, imap_config_from_env,
    imap_config_from_servers, keychain_entry, remember_password,
};
use demo::{decorate_demo_store, reseed_demo_if_stale, spawn_demo_seeding};
use diag::{DIAG, SELFTEST, data_dir, log_sync};
use message_view::ViewTokens;
use state::{AppState, now_ms};
use sync::spawn_real_sync;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// The password out of an environment-driven config.
///
/// That account is always a password — it is built from `PETREL_IMAP_PASS` —
/// so the other arm cannot happen. Written as a match anyway rather than an
/// unwrap, because the compiler will point here the day it can.
fn env_pass(cfg: &ImapConfig) -> &str {
    match &cfg.credential {
        petrel_providers::imap::Credential::Password(p) => p,
        petrel_providers::imap::Credential::Bearer(_) => "",
    }
}

/// Stamps `data-theme` on the document, as early as anything can.
///
/// Its own function so it can be tested: this runs before every other script
/// in the window, so a syntax error here would not break the theme, it would
/// break the app — and it is exactly the kind of string that survives review
/// by looking plausible.
///
/// `theme` arrives already quoted as JSON. `documentElement` normally exists
/// by the time an init script runs, but the retry costs nothing and the
/// alternative is a theme that silently fails to apply on whichever platform
/// disagrees.
fn theme_init_script(theme: &str) -> String {
    format!(
        "(function(){{var t={theme};function a(){{\
         if(document.documentElement){{document.documentElement.setAttribute('data-theme',t);}}\
         else{{requestAnimationFrame(a);}}}}a();}})();"
    )
}

/// The theme the person chose, if it is not "system".
///
/// Only light and dark are worth stamping: "system" means letting the
/// stylesheet's own `prefers-color-scheme` block decide, which it already
/// does correctly and without an attribute. Returned as a JSON string so it
/// can be dropped straight into the script.
fn stored_theme(state: &std::sync::Arc<AppState>) -> Option<String> {
    let store = state.store.lock().ok()?;
    let settings = store.settings().ok()?;
    match settings.get("theme").map(String::as_str) {
        Some("dark") => Some("\"dark\"".into()),
        Some("light") => Some("\"light\"".into()),
        _ => None,
    }
}

/// What a failed launch says: the thing that would not open, where it is,
/// and what the operating system said about it.
///
/// Its own function so the words can be tested. Everything it describes
/// happens before any window exists, and the message is the only thing a
/// person will ever see about it.
fn startup_message(what: &str, path: &std::path::Path, error: &str) -> String {
    format!(
        "Petrel could not start.\n\n{what}\n\n{}\n\n{error}",
        path.display()
    )
}

/// Says so, natively, and leaves.
///
/// These failures used to be `expect`s. A release build has no console
/// attached, so a data directory that could not be opened — a full disk, a
/// permission the OS revoked, a store on a volume that is no longer mounted —
/// was an icon that bounced once in the Dock and vanished, which looks
/// exactly like a crash with no cause. The dialog is native because there is
/// no window yet to put one in.
fn cannot_start(what: &str, path: &std::path::Path, error: &str) -> ! {
    let message = startup_message(what, path, error);
    eprintln!("[startup] {message}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Petrel could not start")
        .set_description(&message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    std::process::exit(1);
}

/// Where the app's own window may navigate.
///
/// Everything Petrel shows is either its bundled page or a message document,
/// and both live at origins this app serves. Left open — which is what a
/// handler returning `true` is — the window navigates anywhere: a file
/// dropped outside a composer replaced the entire app with whatever the file
/// happened to be, and a link that got past the reading frame's own
/// interceptor would have loaded a live web page where the mail client was.
pub(crate) fn main_navigation_allowed(url: &tauri::Url) -> bool {
    let allowed = app_origin(url) || message_origin_url(url);
    if !allowed {
        // The origin, never the whole URL: a message's links are the
        // sender's text and a query string can carry anything.
        log_sync(&format!("[nav] refused {}", origin_of(url)));
    }
    allowed
}

/// Where the print window may navigate: the message it was opened on, and
/// nothing else. Its document catches link clicks itself; this is the
/// webview refusing whatever gets past that.
pub(crate) fn print_navigation_allowed(url: &tauri::Url) -> bool {
    let allowed = message_origin_url(url);
    if !allowed {
        log_sync(&format!("[nav] print window refused {}", origin_of(url)));
    }
    allowed
}

/// The app's own page, in every spelling the platforms give it — plus the
/// dev server, which only a development build ever loads.
fn app_origin(url: &tauri::Url) -> bool {
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        "tauri" => host == "localhost",
        "http" | "https" => {
            host == "tauri.localhost" || (cfg!(dev) && matches!(host, "localhost" | "127.0.0.1"))
        }
        // A webview navigates itself to about:blank while it starts; refusing
        // that refuses the window.
        "about" => true,
        _ => false,
    }
}

/// A message document, in both spellings — see `message_view::message_origin`.
fn message_origin_url(url: &tauri::Url) -> bool {
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        "petrel-msg" => host == "localhost",
        "http" | "https" => host == "petrel-msg.localhost",
        _ => false,
    }
}

/// Scheme and host alone, for the log.
fn origin_of(url: &tauri::Url) -> String {
    format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""))
}

/// Clears out what previous runs left in the data directory.
///
/// Two kinds of leftovers. A "Remove all local data" renames the old
/// directory aside and quits — the store is still open at that moment, and
/// on Windows an open SQLite file cannot be deleted — so the deletion
/// happens here, on the next launch, when nothing holds it. And the staging
/// directory holds copies of attachments that were dropped into a composer:
/// they are somebody's mail, they are only needed until the message goes,
/// and nothing ever removed them.
fn sweep_leftovers(dir: &std::path::Path) {
    let removed = commands::storage::purge_removed(dir);
    if removed > 0 {
        log_sync("mail from an earlier removal request deleted");
    }
    // Staged attachments: copies of somebody's mail, written when a file is
    // dragged into a composer, and nothing ever removed them. Old ones only
    // — a draft written last night and sent this morning must still find
    // what was attached to it, and a fortnight is far longer than a message
    // spends being written.
    let staged = dir.join("staged");
    let _ = crate::diag::create_private_dir(&staged);
    const KEEP: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 60 * 60);
    if let Ok(entries) = std::fs::read_dir(&staged) {
        let mut swept = 0usize;
        for entry in entries.flatten() {
            let old_enough = entry
                .metadata()
                .and_then(|m| m.modified())
                .and_then(|t| t.elapsed().map_err(std::io::Error::other))
                .map(|age| age > KEEP)
                .unwrap_or(false);
            if old_enough && std::fs::remove_file(entry.path()).is_ok() {
                swept += 1;
            }
        }
        if swept > 0 {
            log_sync(&format!("{swept} staged attachment(s) cleared"));
        }
    }
    // And the per-launch directories attachments are opened from, which live
    // in the system temp directory — world-readable on Linux. Only this
    // app's, and never this launch's own: two Petrels at once must not
    // delete each other's open files.
    let mine = format!("petrel-open-{}", std::process::id());
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("petrel-open-") && name != mine {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

pub fn run() {
    // Chosen once, here, for the whole process. rustls picks a crypto provider
    // on its own only while exactly one is compiled in; the moment a second
    // dependency brought another, every TLS handshake that ran before the sync
    // had warmed one up panicked — which is to say, the onboarding connection
    // test on a first run. An application is supposed to say which it wants.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        cannot_start("Its folder could not be created.", &dir, &e.to_string());
    }
    eprintln!("[store] data directory: {}", dir.display());
    // Anything a previous "Remove all local data" renamed aside, and the
    // working directories the last run left behind.
    sweep_leftovers(&dir);

    let db = dir.join("petrel.db");
    let store = match Store::open(&db) {
        Ok(s) => s,
        Err(e) => cannot_start("Its mailbox could not be opened.", &db, &e.to_string()),
    };
    // One account row for now; the account model arrives with setup UI.
    let first = match store.first_account() {
        Ok(a) => a,
        Err(e) => cannot_start("Its mailbox could not be read.", &db, &e.to_string()),
    };
    let account = match first {
        Some(id) => id,
        None => match store.ensure_test_account() {
            Ok(id) => id,
            Err(e) => cannot_start("Its mailbox could not be written to.", &db, &e.to_string()),
        },
    };
    // Name it after the account actually configured, before any sync runs — the
    // address is known from the environment, so there is no reason for the
    // switcher to say test@example.com while real mail is arriving.
    if let Some(cfg) = imap_config_from_env()
        && let Err(e) = store.set_account_email(account, &cfg.user)
    {
        eprintln!("[store] could not name the account: {e}");
    }
    // Accounts made before colours were assigned at creation wear none, and
    // show as grey dots nobody can tell apart. Each gets the next free one.
    for id in store.account_ids().unwrap_or_default() {
        let _ = store.ensure_account_colour(id);
    }
    let blob_dir = dir.join("blobs");
    let blobs = match BlobStore::open(&blob_dir) {
        Ok(b) => b,
        Err(e) => cannot_start(
            "Its message files could not be opened.",
            &blob_dir,
            &e.to_string(),
        ),
    };

    // Startup housekeeping: clear temp files left by an interrupted write, then
    // destroy anything whose grace period expired while the app was closed.
    let _ = blobs.sweep_tmp();
    let state = Arc::new(AppState {
        store: Mutex::new(store),
        blobs,
        seeding: AtomicBool::new(true),
        demo: AtomicBool::new(false),
        seeded: AtomicUsize::new(0),
        status_count: AtomicUsize::new(0),
        source: Mutex::new("starting…".into()),
        sync_error: Mutex::new(None),
        stops: Mutex::new(std::collections::HashMap::new()),
        picked: Mutex::new(std::collections::HashSet::new()),
        caps: Mutex::new(std::collections::HashMap::new()),
        outbox: Mutex::new(Vec::new()),
        draining: AtomicBool::new(false),
        draft_dirty: Mutex::new(std::collections::HashSet::new()),
        pending_notify: Mutex::new(Vec::new()),
        pending_alerts: Mutex::new(Vec::new()),
        last_sync_ms: std::sync::atomic::AtomicI64::new(0),
        ui_touch_ms: std::sync::atomic::AtomicI64::new(0),
        server_total: std::sync::atomic::AtomicUsize::new(0),
        shown_once: Mutex::new(std::collections::HashSet::new()),
        tokens: Arc::new(ViewTokens::new()),
        account_id: account,
        data_dir: dir.display().to_string(),
    });

    // Maintenance never stands between launch and the window. This ran
    // inline here once, and the day the store crossed twenty-eight thousand
    // messages the orphan sweep took minutes — on the main thread, before
    // the window existed, while the sync workers below ran happily and made
    // the process look alive. The window comes up first; the sweep runs a
    // little later, off-thread, and (indexed, migration 0016) holds the
    // store lock for milliseconds.
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            let now = now_ms();
            if let Ok(mut store) = state.store.lock() {
                match store.gc(
                    &state.blobs,
                    now,
                    petrel_engine::retention::DEFAULT_GRACE_DAYS,
                ) {
                    Ok(r)
                        if r.messages_purged > 0
                            || r.blobs_removed > 0
                            || r.actions_orphaned > 0 =>
                    {
                        eprintln!(
                            "[store] gc purged {} message(s), reclaimed {} blob(s), retired {} orphaned action(s)",
                            r.messages_purged, r.blobs_removed, r.actions_orphaned
                        )
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("[store] gc failed: {e}"),
                }
            }
        });
    }

    // Accounts and sync start off the launch path, for the same reason the
    // garbage collector did: nothing here may stand between launch and the
    // window. Reading a password is the worst offender — on a build macOS
    // has not seen before, the keychain blocks on a consent dialog, and
    // that dialog used to hold the whole startup hostage with no window
    // behind it. Now the window comes up first and the prompt, when there
    // is one, appears over a running app.
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            // Which store this is, before any password is read: keychain items
            // are named per store, and a read under the wrong name is a
            // consent dialog for nothing.
            let account_ids = match state.store.lock() {
                Ok(store) => {
                    adopt_store_identity(&store);
                    // A send that was mid-SMTP when the app last quit is
                    // still marked Transmitting, which nothing picks up and no
                    // button moves. Held for a person here, before any worker
                    // exists, so recovery cannot mistake a fresh attempt for
                    // a stale one.
                    match store.recover_interrupted_sends() {
                        Ok(n) if n > 0 => {
                            log_sync(&format!("recovered {n} send(s) interrupted in-flight"))
                        }
                        Ok(_) => {}
                        Err(e) => log_sync(&format!("could not recover interrupted sends: {e}")),
                    }
                    store.account_ids().unwrap_or_default()
                }
                Err(_) => Vec::new(),
            };
            // Keychain after the lock is dropped. A cargo-run binary is a new
            // "app" every time; get_password then blocks on a consent dialog
            // that often never surfaces, and every IPC command that needs the
            // store — including the list — waits behind it.
            adopt_legacy_keychain_items(&account_ids);
            // The account set up in the app first; the environment as the developer
            // override when there is none. Before this every launch without the
            // variables was a demo — which is how demo tags ended up decorating a
            // store full of real mail.
            // Every account set up in the app syncs, each on its own tasks. One is
            // *shown* at a time — that is the switcher's job — but mail arriving for
            // the other should be there, read or not, the moment you switch to it.
            // The environment-driven row is the fallback for the developer case only.
            let mut started = 0;
            // Adoption: an account row created from environment variables before
            // onboarding existed has no stored servers and no keychain entry — and
            // once a second account *is* properly configured, the env fallback below
            // never fires again, so that first account silently stops syncing
            // (found exactly that way: Gmail dead for days behind a working
            // Namecheap). If the environment names such an account's own address,
            // its credentials move into the keychain now, once, and it becomes an
            // ordinary configured account.
            if let Some(env) = imap_config_from_env() {
                let adopted: Vec<i64> = match state.store.lock() {
                    Ok(store) => {
                        let mut ids = Vec::new();
                        for summary in store.accounts().unwrap_or_default() {
                            let stored = store
                                .account_servers(summary.id)
                                .ok()
                                .flatten()
                                .is_some_and(|s| !s.imap_host.is_empty());
                            if stored || !summary.email.eq_ignore_ascii_case(&env.user) {
                                continue;
                            }
                            let smtp = petrel_providers::smtp::SmtpConfig::for_imap_host(
                                &env.host,
                                &env.user,
                                env_pass(&env),
                            );
                            let servers = petrel_engine::store::AccountServers {
                                imap_host: env.host.clone(),
                                imap_port: env.port,
                                smtp_host: smtp.host,
                                smtp_port: smtp.port,
                                username: env.user.clone(),
                                provider: String::new(),
                            };
                            if let Err(e) = store.set_account_servers(summary.id, &servers) {
                                eprintln!(
                                    "[sync] could not adopt env servers for account {}: {e}",
                                    summary.id
                                );
                                continue;
                            }
                            ids.push(summary.id);
                        }
                        ids
                    }
                    Err(_) => Vec::new(),
                };
                for id in adopted {
                    if let Ok(entry) = keychain_entry(id) {
                        // set_password refuses to overwrite on macOS; clear first.
                        let _ = entry.delete_credential();
                        match entry.set_password(env_pass(&env)) {
                            Ok(()) => {
                                remember_password(id, env_pass(&env));
                                log_sync(&format!(
                                    "adopted environment credentials for account {id} into the keychain"
                                ));
                            }
                            Err(e) => eprintln!("[sync] keychain adopt failed: {e}"),
                        }
                    }
                }
            }
            let server_rows: Vec<(i64, petrel_engine::store::AccountServers)> = state
                .store
                .lock()
                .ok()
                .map(|s| {
                    s.account_ids()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|id| {
                            let servers = s.account_servers(id).ok().flatten()?;
                            (!servers.imap_host.is_empty()).then_some((id, servers))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let configs: Vec<(i64, ImapConfig)> = server_rows
                .into_iter()
                .filter_map(|(id, servers)| imap_config_from_servers(id, servers).map(|c| (id, c)))
                .collect();
            // Re-own the keychain items, once. A keychain item remembers the
            // app that created it, and these were created by ad-hoc builds —
            // a different "app" every rebuild — so even the signed build had
            // to ask on every launch. Rewriting each item with the same
            // secret makes the signed identity the creator, and that
            // identity is stable now, so the asking ends here. The passwords
            // are already in hand from the reads above; marker-gated so this
            // runs once per signing identity, not per launch.
            {
                const REOWN_MARKER: &str = "petrel-dev-9e1c62a7";
                let owned = state
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.settings().ok())
                    .and_then(|s| s.get("keychain_reowned").cloned())
                    .unwrap_or_default();
                if owned != REOWN_MARKER && !configs.is_empty() {
                    let mut all_ok = true;
                    for (id, cfg) in &configs {
                        if let Ok(entry) = keychain_entry(*id) {
                            let _ = entry.delete_credential();
                            if let Err(e) = entry.set_password(env_pass(cfg)) {
                                eprintln!("[keychain] re-own of account {id} failed: {e}");
                                all_ok = false;
                            }
                        }
                    }
                    if all_ok && let Ok(store) = state.store.lock() {
                        let _ = store.set_setting("keychain_reowned", REOWN_MARKER);
                        log_sync("keychain items re-owned by the signed build");
                    }
                }
            }
            for (id, cfg) in configs {
                eprintln!(
                    "[sync] account {id} configured: {} @ {}",
                    cfg.user, cfg.host
                );
                spawn_real_sync(state.clone(), id, cfg);
                started += 1;
            }
            let configured = if started > 0 {
                None
            } else {
                imap_config_from_env()
            };
            // The "N so far" figure starts from what is already here, whichever branch
            // runs. It used to start at zero and be pushed along by every fetch; once
            // the counter learned to count only genuinely new mail, a relaunch that
            // re-fetches stored folders moved it not at all — and an empty folder
            // showed "Fetching your mail — 0 so far…" over a store holding thousands.
            {
                let existing = state
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.message_count().ok())
                    .unwrap_or(0);
                state.seeded.store(existing as usize, Ordering::Relaxed);
            }
            match (started, configured) {
                (n, _) if n > 0 => {}
                (_, Some(cfg)) => {
                    eprintln!("[sync] account configured: {} @ {}", cfg.user, cfg.host);
                    spawn_real_sync(state.clone(), account, cfg);
                }
                (_, None) => {
                    // No account anywhere: whatever is on screen is synthetic,
                    // and the window is told so rather than being left to infer
                    // a first run from the absence of one.
                    state.demo.store(true, Ordering::Relaxed);
                    // Demo data is for an empty first run only. Seeding it into a store
                    // that already holds real mail would mix fabricated messages into
                    // someone's actual mailbox — found the hard way when a persistence
                    // test relaunched without credentials and buried a real message
                    // under 10,000 synthetic ones.
                    let existing = state
                        .store
                        .lock()
                        .ok()
                        .and_then(|s| s.message_count().ok())
                        .unwrap_or(0);
                    if existing > 0 {
                        state.seeded.store(existing as usize, Ordering::Relaxed);
                        state.seeding.store(false, Ordering::Relaxed);
                        *state.source.lock().unwrap_or_else(|p| p.into_inner()) =
                            "no account configured · showing stored mail".into();
                        if !reseed_demo_if_stale(&state, account) {
                            decorate_demo_store(&state, account);
                        }
                    } else {
                        *state.source.lock().unwrap_or_else(|p| p.into_inner()) =
                            "synthetic demo data".into();
                        spawn_demo_seeding(state.clone(), account);
                    }
                }
            }
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::settings::status,
            commands::mail::list_threads,
            commands::mail::thread_by_id,
            commands::windows::open_external,
            commands::compose::stage_attachment,
            commands::mail::list_tags,
            commands::mail::view_counts,
            commands::remote::remote_status,
            commands::remote::show_remote_once,
            commands::remote::trust_sender,
            commands::remote::trusted_senders,
            commands::remote::untrust_sender,
            commands::mail::thread_detail,
            commands::mail::thread_index,
            commands::mail::thread_message,
            commands::triage::triage,
            commands::triage::undo_triage,
            commands::triage::list_folders,
            commands::triage::create_folder,
            commands::triage::rename_folder,
            commands::triage::delete_folder,
            commands::triage::empty_trash,
            commands::triage::folder_message_count,
            commands::triage::mark_folder_read,
            commands::triage::trash_folder_contents,
            commands::triage::create_tag,
            commands::triage::rename_tag,
            commands::triage::set_tag_colour,
            commands::triage::delete_tag,
            commands::accounts::discover_account,
            commands::accounts::guess_servers,
            commands::accounts::test_account,
            commands::accounts::add_account,
            commands::accounts::remove_account,
            commands::accounts::set_active_account,
            commands::attachments::attachment_is_executable,
            commands::attachments::save_attachment,
            commands::attachments::open_attachment,
            commands::attachments::attachment_url,
            commands::outbox::list_outbox,
            commands::outbox::outbox_send_now,
            commands::outbox::outbox_edit,
            commands::outbox::outbox_check,
            commands::storage::storage_report,
            commands::storage::removal_report,
            commands::storage::remove_all_local_data,
            commands::storage::export_mbox,
            commands::storage::import_mail,
            commands::mail::print_message,
            commands::mail::view_count,
            commands::settings::post_notification,
            commands::settings::list_rules,
            commands::settings::save_rule,
            commands::settings::delete_rule,
            commands::settings::move_rule,
            commands::updates::app_version,
            commands::updates::check_update,
            commands::updates::install_update,
            commands::updates::restart_for_update,
            commands::settings::export_settings,
            commands::settings::import_settings,
            commands::compose::get_identity,
            commands::compose::set_identity,
            commands::compose::attachment_info,
            commands::compose::schedule_send,
            commands::windows::popout_compose,
            commands::windows::popout_message,
            commands::windows::set_dock_badge,
            commands::triage::reorder_folders,
            commands::triage::reorder_tags,
            commands::compose::complete_addresses,
            commands::compose::quote_message,
            commands::compose::save_draft,
            commands::compose::push_draft,
            commands::remote::unsubscribe_info,
            commands::remote::authentication_info,
            commands::remote::unsubscribe_one_click,
            commands::compose::draft_conflict,
            commands::compose::resolve_draft_conflict,
            commands::compose::load_draft,
            commands::compose::delete_draft,
            commands::accounts::list_accounts,
            commands::accounts::set_account_color,
            commands::accounts::set_account_archive,
            commands::settings::get_settings,
            commands::settings::set_setting,
            commands::mail::search_messages,
            commands::mail::message_url,
            commands::files::pick_files,
            commands::files::pick_save_path,
            commands::invitations::invitation,
            commands::invitations::respond_invitation,
            diag::frontend_log
        ])
        .register_uri_scheme_protocol("petrel-msg", move |ctx, request| {
            #[cfg(debug_assertions)]
            if request.uri().path().starts_with("/doc/")
                || request.uri().path().starts_with("/beacon/")
            {
                return spike_s2::handle(&request);
            }
            let state = ctx.app_handle().state::<Arc<AppState>>();
            let lookup = |id: i64| {
                state
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.blob_hash_for(id).ok().flatten())
            };
            // Read per request, so changing the setting takes effect on the
            // next message rather than the next launch. Anything unreadable
            // falls back to blocking: the safe answer is the one a failure
            // should produce.
            // Three ways a message earns its remote content, checked in the
            // order that costs least: the user turned blocking off entirely,
            // they asked to see this one message, or the sender is someone the
            // engine already trusts. Any failure along the way blocks.
            let policy_state = Arc::clone(&state);
            let blocking_off = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.settings().ok())
                .map(|s| s.get("blockRemoteContent").map(String::as_str) == Some("off"))
                .unwrap_or(false);
            let allow_remote = move |message_id: i64| {
                if blocking_off {
                    return true;
                }
                if policy_state
                    .shown_once
                    .lock()
                    .map(|set| set.contains(&message_id))
                    .unwrap_or(false)
                {
                    return true;
                }
                policy_state
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.remote_content_allowed(message_id).ok())
                    .unwrap_or(false)
            };
            message_view::handle(&request, &state.tokens, &state.blobs, lookup, allow_remote)
        })
        .setup(|app| {
            // PETREL_MINIMAL=1: a bare window with none of our machinery — no
            // init script, no custom protocol, no state. If this is also dead,
            // the problem is the platform/Tauri pairing, not this app.
            if std::env::var("PETREL_MINIMAL").is_ok() {
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
                tauri::WebviewWindowBuilder::new(
                    app,
                    "main",
                    tauri::WebviewUrl::App("minimal.html".into()),
                )
                .title("minimal")
                .inner_size(700.0, 400.0)
                .build()?;
                return Ok(());
            }

            let mut init = DIAG.to_string();
            // The chosen theme, stamped before the page's own scripts run.
            //
            // The renderer reads its settings over IPC, which is a round trip
            // that finishes well after the first frame — so a person who
            // picked dark on a light machine, or light on a dark one, saw the
            // opposite theme flash on every single launch. Nobody noticed
            // because the default is "system", which matches whatever the
            // stylesheet would have done anyway.
            //
            // It cannot be an inline script in index.html: the CSP is
            // `script-src 'self'`. An init script is exempt and runs earlier,
            // which is exactly what this needs.
            if let Some(theme) = stored_theme(&app.state::<Arc<AppState>>()) {
                init.push_str(&theme_init_script(&theme));
            }
            if let Ok(mode) = std::env::var("PETREL_SELFTEST") {
                if mode == "open" {
                    init.push_str(
                        "window.__PETREL_SELFTEST_QUERIES__=['hostile'];\
                         window.__PETREL_SELFTEST_OPEN__=true;",
                    );
                }
                init.push_str(SELFTEST);
            }
            #[cfg(debug_assertions)]
            if std::env::var("PETREL_SPIKE_S2").is_ok() {
                let port = spike_s2::start_leak_listener();
                eprintln!("[s2] leak listener on 127.0.0.1:{port}");
                init.push_str("window.__PETREL_SPIKE__='s2';");
            }
            // Run un-bundled (bundle.active = false), macOS gives the process an
            // accessory activation policy: the window draws and can be dragged,
            // but never becomes key, so hover and clicks inside the webview go
            // nowhere. Ask for Regular explicitly.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
                .title("Petrel")
                // Tauri's own drag-and-drop handler registers a native drop
                // target over the whole webview, and on Windows that stops the
                // page's own HTML5 drag events from ever firing. Dragging a
                // conversation onto a mailbox is a page-level gesture, so the
                // page has to be the one hearing it.
                //
                // Nothing is lost by turning it off: we accept no OS file
                // drops today, and when compose learns to take an attachment
                // that way it arrives as `dataTransfer.files` on the same
                // HTML5 drop event this enables.
                .disable_drag_drop_handler()
                .inner_size(1440.0, 900.0)
                .min_inner_size(900.0, 560.0)
                .position(40.0, 40.0)
                .focused(true)
                .initialization_script(&init)
                .on_navigation(main_navigation_allowed)
                .on_page_load(|_webview, payload| {
                    log_sync(&format!(
                        "[pageload] {:?} {}",
                        payload.event(),
                        payload.url()
                    ));
                })
                .build()?;

            // Say where it actually landed — a window that opens behind another
            // app looks identical to one that failed to open.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
                if let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) {
                    log_sync(&format!(
                        "[window] main at {},{} size {}x{}",
                        pos.x, pos.y, size.width, size.height
                    ));
                }
            }
            // PETREL_UPDATE_PROBE=check|install: drive the update path from
            // the running app and report through the log, so a release can be
            // verified end to end without a person clicking a button in a
            // window no test can reach.
            if let Ok(mode) = std::env::var("PETREL_UPDATE_PROBE") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    match commands::updates::check_update(handle.clone()).await {
                        Ok(status) => log_sync(&format!(
                            "update probe: current={} available={:?} error={:?}",
                            status.current, status.available, status.error
                        )),
                        Err(e) => log_sync(&format!("update probe: check failed: {e}")),
                    }
                    if mode == "install" {
                        match commands::updates::install_update(handle).await {
                            Ok(()) => log_sync("update probe: installed"),
                            Err(e) => log_sync(&format!("update probe: install failed: {e}")),
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running petrel");
}

#[cfg(test)]
mod theme_init_tests {
    use super::theme_init_script;

    /// This script runs before every other script in the window. A mistake in
    /// it does not produce the wrong theme, it produces a window where nothing
    /// runs at all — so the shape is checked rather than assumed.
    #[test]
    fn it_stamps_the_theme_it_was_given() {
        let js = theme_init_script("\"dark\"");
        assert!(js.contains("setAttribute('data-theme',t)"), "{js}");
        assert!(js.contains("var t=\"dark\";"), "{js}");
    }

    #[test]
    fn the_braces_balance_and_nothing_is_left_open() {
        // A line-continuation in the Rust literal eating a brace is the exact
        // way this breaks, and it reads as fine.
        for theme in ["\"dark\"", "\"light\""] {
            let js = theme_init_script(theme);
            let opens = js.matches('{').count();
            let closes = js.matches('}').count();
            assert_eq!(opens, closes, "unbalanced braces for {theme}: {js}");
            assert_eq!(js.matches('(').count(), js.matches(')').count(), "{js}");
            assert!(js.ends_with("})();"), "{js}");
            // One statement, one line: a stray newline inside would be
            // harmless, but a stray backslash would not.
            assert!(
                !js.contains('\\'),
                "a backslash survived into the script: {js}"
            );
        }
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{main_navigation_allowed, print_navigation_allowed, startup_message};

    /// The message is the only thing a person sees when the app cannot
    /// start, so it has to say which file and what went wrong. Before this
    /// the `expect` printed to a console a release build does not have.
    #[test]
    fn a_failed_launch_names_the_file_and_the_reason() {
        let message = startup_message(
            "Its mailbox could not be opened.",
            std::path::Path::new("/Users/x/Library/Application Support/Petrel/petrel.db"),
            "unable to open database file",
        );
        assert!(message.contains("Petrel could not start."), "{message}");
        assert!(
            message.contains("Its mailbox could not be opened."),
            "{message}"
        );
        assert!(message.contains("Petrel/petrel.db"), "{message}");
        assert!(
            message.contains("unable to open database file"),
            "{message}"
        );
    }

    fn url(s: &str) -> tauri::Url {
        s.parse().expect("a url")
    }

    /// The window may go to its own page and to a message, and nowhere else.
    /// A file dropped outside a composer used to replace the whole app.
    #[test]
    fn the_main_window_navigates_only_to_petrels_own_origins() {
        for allowed in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/index.html?compose=3",
            "https://tauri.localhost/index.html",
            "petrel-msg://localhost/message/abc",
            "http://petrel-msg.localhost/message/abc",
        ] {
            assert!(main_navigation_allowed(&url(allowed)), "{allowed}");
        }
        for refused in [
            "file:///Users/x/Desktop/invoice.pdf",
            "https://example.com/",
            "http://petrel-msg.localhost.example.com/",
            "https://tauri.localhost.evil.example/",
            "data:text/html,<script>alert(1)</script>",
            "javascript:alert(1)",
            "petrel-msg://elsewhere/message/abc",
        ] {
            assert!(!main_navigation_allowed(&url(refused)), "{refused}");
        }
    }

    /// The print window shows one message and never anything else, so a
    /// link in that message cannot load a web page in a Petrel window.
    #[test]
    fn the_print_window_navigates_only_to_a_message() {
        assert!(print_navigation_allowed(&url(
            "petrel-msg://localhost/print/tok"
        )));
        assert!(print_navigation_allowed(&url(
            "http://petrel-msg.localhost/print/tok"
        )));
        for refused in [
            "https://example.com/",
            "tauri://localhost/index.html",
            "file:///etc/passwd",
        ] {
            assert!(!print_navigation_allowed(&url(refused)), "{refused}");
        }
    }
}
