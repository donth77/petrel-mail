//! Petrel's desktop shell: a thin window over the engine. All real work
//! happens in `petrel-engine`; this crate wires typed IPC and (soon) the
//! `petrel-msg://` custom protocol for sanitized message documents.
//!
//! Two source modes: with `PETREL_IMAP_*` set it syncs a real mailbox through
//! the engine's ingest path; without, it seeds synthetic mail so the UI is
//! exercisable with no account. Both run the same store, index, and queries.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use petrel_engine::actions::{ActionKind, ActionReceipt};
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{
    AccountSummary, DraftRecord, FolderSummary, Identity, ListView, Listing, NewMessage,
    StorageReport, Store, TagSummary, ThreadListing, ThreadMessage,
};
use petrel_providers::imap::{ImapConfig, Security};
use petrel_testkit::DemoMailbox;
use tauri::{Manager, State};

// Public so the render path can be tested directly. The privacy guarantees
// live in this module, and they are worth asserting on rather than trusting.
pub mod message_view;
mod spike_s2;

use message_view::ViewTokens;

const DEMO_MESSAGES: usize = 10_000;

struct AppState {
    store: Mutex<Store>,
    blobs: BlobStore,
    seeding: AtomicBool,
    seeded: AtomicUsize,
    source: Mutex<String>,
    /// Set when a sync fails. Separate from `source` because a failure has to
    /// reach the screen, and `source` is a label the UI is free to ignore —
    /// which is exactly what it did, leaving a failed login looking like an
    /// empty mailbox.
    sync_error: Mutex<Option<String>>,
    /// Raised when local triage is waiting to reach the server. The drain
    /// worker sleeps on this rather than on a timer, so an archive reaches
    /// Gmail in about a second instead of whenever the next sync happens to
    /// come round — which, with IDLE holding a connection open, could be
    /// twenty minutes.
    drain_signal: Arc<tokio::sync::Notify>,
    /// One drain at a time. Two overlapping passes would both read the same
    /// queued rows and deliver each change twice.
    draining: AtomicBool,
    /// Drafts edited since their last push to the server, for the 30-second
    /// debounce. A draft in here has exactly one push task sleeping on it.
    draft_dirty: Mutex<std::collections::HashSet<i64>>,
    /// Whether the server supports UID MOVE, learned from the probe.
    server_has_move: AtomicBool,
    /// RFC 4315. Without it a message can be marked deleted but not expunged,
    /// because a bare EXPUNGE would take every other \\Deleted message with it.
    server_has_uidplus: AtomicBool,
    /// Whether this account's tags are Gmail labels rather than IMAP keywords.
    server_is_gmail: AtomicBool,
    /// How much mail the server says it holds, across the folders we sync.
    ///
    /// The denominator of the coverage line, and the reason it exists: a client
    /// that quietly returns three results out of a possible ten teaches you not
    /// to trust its search. Zero until a sync has asked.
    server_total: std::sync::atomic::AtomicUsize,
    /// Messages the user asked to see this once. Deliberately not persisted:
    /// "show images" is a decision about one message on one occasion, and a
    /// version of it that outlived the session would be trust nobody granted.
    shown_once: Mutex<std::collections::HashSet<i64>>,
    tokens: Arc<ViewTokens>,
    account_id: i64,
    data_dir: String,
}

#[derive(serde::Serialize)]
struct Status {
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
}

/// Turns a protocol error into something a person can act on.
///
/// The raw text is Rust's Debug rendering of an IMAP response — `code: None,
/// info: Some("[AUTHENTICATIONFAILED] ...")` — which tells a user nothing and
/// tells them it unhelpfully. The detail still goes to sync.log; what reaches
/// the screen should say what to do about it.
fn friendly_sync_error(raw: &str) -> String {
    let r = raw.to_ascii_uppercase();
    if r.contains("AUTHENTICATIONFAILED") || r.contains("INVALID CREDENTIALS") {
        return "Sign-in was refused. Gmail needs 2-Step Verification switched on \
                and an app password — your ordinary account password will not work \
                for IMAP."
            .into();
    }
    if r.contains("AUTHORIZATIONFAILED") || r.contains("WEBALERT") {
        return "The server accepted the password but refused access. For Gmail \
                this usually means IMAP is switched off in settings."
            .into();
    }
    if r.contains("DNS") || r.contains("NAME OR SERVICE") || r.contains("RESOLVE") {
        return "That server name could not be looked up. Check the host.".into();
    }
    if r.contains("CONNECTION REFUSED") || r.contains("TIMED OUT") || r.contains("TIMEOUT") {
        return "The server did not answer. Check the host and port, and whether \
                something on this network blocks IMAP."
            .into();
    }
    if r.contains("CERTIFICATE") || r.contains("TLS") || r.contains("HANDSHAKE") {
        return "The encrypted connection could not be established, so Petrel \
                stopped rather than continuing in the clear."
            .into();
    }
    // Unknown: show the raw text rather than a reassuring guess.
    raw.to_string()
}

/// Appends a line to a log file in the data directory.
///
/// Under LaunchServices — which is the only way the app gets real keyboard
/// focus on macOS — stderr goes nowhere readable, so `eprintln!` diagnostics
/// vanish precisely when the app is being run the way a user runs it. Anything
/// worth printing during a sync is worth writing here.
fn log_sync(msg: &str) {
    eprintln!("[sync] {msg}");
    let path = data_dir().join("sync.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{} {msg}", now_ms());
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command]
fn status(state: State<Arc<AppState>>) -> Status {
    let configured = state
        .store
        .lock()
        .ok()
        .and_then(|s| {
            s.active_account()
                .ok()
                .flatten()
                .map(|a| imap_config_for(&s, a).is_some())
        })
        .unwrap_or(false)
        || imap_config_from_env().is_some();
    Status {
        configured,
        seeding: state.seeding.load(Ordering::Relaxed),
        count: state.seeded.load(Ordering::Relaxed),
        server_total: state.server_total.load(Ordering::Relaxed),
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

/// Reads account settings from the environment. Credentials never appear in
/// argv (visible to every process on the machine) or in a config file we wrote;
/// the keychain replaces this at M4 when account setup exists.
/// The keychain entry for an account's password.
///
/// Keyed by the account's row id rather than its address, so renaming an
/// account or adding a second one with the same address on another server
/// cannot point two accounts at one secret.
fn keychain_entry(account_id: i64) -> Result<keyring::Entry, String> {
    keyring::Entry::new("dev.petrel.desktop", &format!("account-{account_id}"))
        .map_err(|e| format!("keychain: {e}"))
}

/// The IMAP configuration for an account that was set up in the app.
///
/// The store has the servers; the keychain has the password. Either missing
/// means this account was not set up here — which, today, means it is the
/// developer row driven by the environment, and the caller falls back.
fn imap_config_for(store: &Store, account_id: i64) -> Option<ImapConfig> {
    let servers = store.account_servers(account_id).ok().flatten()?;
    if servers.imap_host.is_empty() {
        return None;
    }
    let pass = keychain_entry(account_id).ok()?.get_password().ok()?;
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
fn smtp_config_for(store: &Store, account_id: i64) -> Option<petrel_providers::smtp::SmtpConfig> {
    let servers = store.account_servers(account_id).ok().flatten()?;
    if servers.smtp_host.is_empty() {
        return None;
    }
    let pass = keychain_entry(account_id).ok()?.get_password().ok()?;
    Some(petrel_providers::smtp::SmtpConfig {
        host: servers.smtp_host,
        port: servers.smtp_port,
        user: servers.username,
        pass,
    })
}

/// The account's IMAP configuration from wherever it lives: the app's own
/// setup first, the environment as the developer override.
fn imap_config(state: &AppState, account_id: i64) -> Option<ImapConfig> {
    state
        .store
        .lock()
        .ok()
        .and_then(|s| imap_config_for(&s, account_id))
        .or_else(imap_config_from_env)
}

fn imap_config_from_env() -> Option<ImapConfig> {
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

/// Delivers queued triage as soon as there is any, rather than when the next
/// sync happens to run.
///
/// Debounced, because triage comes in bursts: working down an inbox is a run of
/// archives a few hundred milliseconds apart, and one connection carrying all
/// of them beats one connection each. A second of latency is invisible to the
/// person doing it and saves a login per keystroke.
fn spawn_drain_worker(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
    tauri::async_runtime::spawn(async move {
        loop {
            state.drain_signal.notified().await;
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            let has_move = state.server_has_move.load(Ordering::Relaxed);
            let has_uidplus = state.server_has_uidplus.load(Ordering::Relaxed);
            drain_actions(
                Arc::clone(&state),
                account,
                cfg.clone(),
                has_move,
                has_uidplus,
                state.server_is_gmail.load(Ordering::Relaxed),
            )
            .await;
            send_due(Arc::clone(&state), account).await;
        }
    });
}

/// Wakes the drain worker when a queued message's time comes.
///
/// Nothing else in the system is clock-driven. The drain runs when a triage
/// action asks for it or when the sync loop comes round — and with IDLE the
/// sync loop sleeps until the server pushes something, so a message scheduled
/// for twenty seconds out waited for *unrelated mail to arrive*. Observed on
/// the live account: due at t+20s, still untouched at t+64s.
///
/// Sleeps to the exact instant rather than polling, so an empty outbox costs
/// nothing; re-checked after every drain, because a drain is what changes the
/// answer. The one-minute cap is for the clock being wrong — a laptop lid
/// closed through the scheduled time — not for accuracy: the send happens on
/// the rung, not a minute late.
fn spawn_outbox_clock(state: Arc<AppState>, account: i64) {
    tauri::async_runtime::spawn(async move {
        loop {
            let next = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.next_due_ms(account).ok())
                .flatten();
            let wait_ms = match next {
                Some(at) => (at - now_ms()).clamp(0, 60_000),
                None => 60_000,
            };
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms as u64)).await;
            if next.is_some_and(|at| at <= now_ms()) {
                state.drain_signal.notify_one();
                // Give the drain its head before asking again, or this loop
                // sees the same due row and fires a second time for nothing.
                tokio::time::sleep(std::time::Duration::from_millis(2_000)).await;
            }
        }
    });
}

/// Clears the draining flag however the drain ends, including on an early
/// return or a panic — a flag left set would silently stop every later drain.
struct DrainGuard(Arc<AppState>);

impl Drop for DrainGuard {
    fn drop(&mut self) {
        self.0.draining.store(false, Ordering::SeqCst);
    }
}

/// Delivers queued triage to the server.
///
/// This is the second half of the optimistic model. Everything the user does is
/// applied locally at once and written to a queue; until something drains that
/// queue, archiving a conversation in Petrel means nothing to anyone else, and
/// the next resync quietly puts it back.
///
/// Order matters and is preserved: two actions on the same message have to
/// arrive the way the user performed them, or the later one loses. A failure
/// stops that action rather than the drain — one unreachable message should not
/// strand every other change behind it — and leaves it queued to retry, because
/// a change that never reached the server is not one to discard.
async fn drain_actions(
    state: Arc<AppState>,
    account: i64,
    cfg: ImapConfig,
    has_move: bool,
    has_uidplus: bool,
    // Whether this account's tags are Gmail labels. Passed in with the other
    // capabilities rather than sniffed here: the probe already worked it out,
    // and two places deciding what a server is would eventually disagree.
    looks_like_gmail: bool,
) {
    use petrel_engine::actions::ActionKind;

    // Refuse to overlap. compare_exchange rather than a load-then-store: two
    // tasks arriving together would both see `false` and both proceed.
    if state
        .draining
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let _guard = DrainGuard(Arc::clone(&state));

    let pending = match state
        .store
        .lock()
        .and_then(|s| Ok(s.pending_actions(account)))
    {
        Ok(Ok(p)) => p,
        _ => return,
    };
    if pending.is_empty() {
        return;
    }
    log_sync(&format!("draining {} queued change(s)", pending.len()));

    let mut delivered = 0usize;
    let mut stuck = 0usize;
    for item in pending {
        let Ok(kind) = serde_json::from_str::<ActionKind>(&item.kind_json) else {
            continue;
        };
        let Some(uid) = item.uid else {
            // Never placed anywhere, so there is no server-side message to act
            // on. Nothing to deliver and nothing to retry.
            continue;
        };
        let folder = if item.folder_path.is_empty() {
            "INBOX".to_string()
        } else {
            item.folder_path.clone()
        };

        let result = match kind {
            ActionKind::MarkRead => {
                petrel_providers::imap::store_flag(&cfg, &folder, uid, "\\Seen", true).await
            }
            ActionKind::MarkUnread => {
                petrel_providers::imap::store_flag(&cfg, &folder, uid, "\\Seen", false).await
            }
            ActionKind::Star => {
                petrel_providers::imap::store_flag(&cfg, &folder, uid, "\\Flagged", true).await
            }
            ActionKind::Unstar => {
                petrel_providers::imap::store_flag(&cfg, &folder, uid, "\\Flagged", false).await
            }
            ActionKind::Archive | ActionKind::Trash | ActionKind::Spam | ActionKind::Move => {
                // The local move has already happened, so the destination is
                // wherever the message now sits: the store is the record of
                // where it should be, and re-deriving it here could disagree.
                let dest = state
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.folders_of(item.message_id).ok())
                    .and_then(|f| f.first().copied())
                    .and_then(|fid| {
                        state
                            .store
                            .lock()
                            .ok()
                            .and_then(|s| s.folder_path(fid).ok().flatten())
                    });
                match dest {
                    Some(to) if to != folder => {
                        match petrel_providers::imap::move_uid(&cfg, &folder, uid, &to, has_move)
                            .await
                        {
                            // A destination the server has never heard of —
                            // the folder was made here moments ago. Create it
                            // and try once more; servers signal this as
                            // TRYCREATE but not all of them say the word.
                            Err(e) => {
                                log_sync(&format!(
                                    "move to {to} failed ({e}); creating and retrying"
                                ));
                                match petrel_providers::imap::create_folder(&cfg, &to).await {
                                    Ok(()) => {
                                        petrel_providers::imap::move_uid(
                                            &cfg, &folder, uid, &to, has_move,
                                        )
                                        .await
                                    }
                                    Err(_) => Err(e),
                                }
                            }
                            ok => ok,
                        }
                    }
                    // Already where it belongs, or nowhere to send it.
                    _ => Ok(()),
                }
            }
            // Local-only, so they should never have been queued at all — the
            // store marks them 'local' and this drain only reads 'queued'.
            // Handled here so adding a local action later cannot silently fall
            // into the tag branch and be counted as stuck forever.
            ActionKind::DeleteForever => {
                // The local row is already a tombstone, so its placements are
                // the last record of where the server copy lives. Expunge from
                // the folder it was queued against.
                match petrel_providers::imap::expunge_uid(&cfg, &folder, uid, has_uidplus).await {
                    // Marked \\Deleted but not expunged: the server has no
                    // UIDPLUS, and a bare EXPUNGE would have committed every
                    // other pending deletion in the mailbox too. Worth a line
                    // in the log, because the message outlives the gesture.
                    Ok(false) => {
                        log_sync(&format!(
                            "{folder}: marked deleted but not expunged (no UIDPLUS)"
                        ));
                        Ok(())
                    }
                    Ok(true) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            ActionKind::Snooze | ActionKind::Unsnooze => continue,
            // A tag is a Gmail label on Gmail, an IMAP keyword elsewhere.
            // Only the first is wired; the rest stay queued rather than being
            // marked done, so they deliver when keywords land instead of being
            // silently dropped.
            ActionKind::Tag | ActionKind::Untag => {
                if !looks_like_gmail {
                    stuck += 1;
                    continue;
                }
                // The action names the tag by id, not by name: a tag can be
                // renamed between queueing and delivery, and the action means
                // "this tag" rather than "whatever it was called at the time".
                let target = serde_json::from_str::<serde_json::Value>(&item.payload_json)
                    .ok()
                    .and_then(|p| p.get("target").and_then(|t| t.as_i64()));
                let name = target.and_then(|id| {
                    state
                        .store
                        .lock()
                        .ok()
                        .and_then(|s| s.tag_name(id).ok())
                        .flatten()
                });
                let Some(name) = name else {
                    // The tag was deleted before its action went out. There is
                    // nothing left to name to the server, and retrying forever
                    // would keep a dead action in the queue.
                    if let Ok(store) = state.store.lock() {
                        let _ = store.mark_action_state(item.action_id, "sent");
                    }
                    continue;
                };
                petrel_providers::imap::store_gmail_labels(
                    &cfg,
                    &folder,
                    uid,
                    &name,
                    matches!(kind, ActionKind::Tag),
                )
                .await
            }
        };

        match result {
            Ok(()) => {
                delivered += 1;
                if let Ok(store) = state.store.lock() {
                    let _ = store.mark_action_state(item.action_id, "sent");
                }
            }
            Err(e) => {
                stuck += 1;
                log_sync(&format!(
                    "action {} could not be delivered: {e}",
                    item.action_id
                ));
            }
        }
    }
    log_sync(&format!(
        "drained {delivered} change(s), {stuck} still queued"
    ));
}

/// One-shot sync: fetch recent mail and ingest it. Deliberately not a sync
/// engine — that arrives with the orchestrator; this proves the path end to end
/// inside the app.
/// The folders worth pulling down, inbox first.
///
/// Deliberately not everything the server advertises:
///
/// * **All Mail is excluded.** On a labels provider it holds *every* message,
///   so syncing it would roughly double the store — and since it is what the
///   archive role maps to, it would make the Archive view mean "all your mail"
///   rather than "mail you archived".
/// * **Starred is included**, despite being a flag we already read. We only
///   read the flags of messages we *fetch* — a star on older mail, or on
///   anything archived into All Mail, never arrives, and the Starred view sits
///   empty while the server knows better. It is small by nature: a list of
///   things someone picked out by hand.
/// * **Snoozed is not here to exclude.** Gmail has the feature, but does not
///   expose it over IMAP — there is no such mailbox in the folder list.
/// * **Outbox likewise**: mail that has not reached a server yet is ours alone.
fn folders_to_sync(state: &AppState, account: i64) -> Vec<(String, String, i64)> {
    let Ok(store) = state.store.lock() else {
        return Vec::new();
    };
    folders_to_sync_from(&store, account)
}

/// The lock-free core, for callers already holding the store.
fn folders_to_sync_from(store: &Store, account: i64) -> Vec<(String, String, i64)> {
    // Inbox first so the view the user is looking at fills before the rest.
    const ROLES: [&str; 6] = ["inbox", "sent", "drafts", "spam", "trash", "starred"];
    let Ok(all) = store.folders(account) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String, i64)> = ROLES
        .iter()
        .filter_map(|role| {
            all.iter()
                .find(|f| f.role == *role)
                .map(|f| ((*role).to_string(), f.path.clone(), f.id))
        })
        .collect();
    // Folders the user made sync too — a folder whose mail never arrives is
    // not a folder, it is a name. After the roles, so the inbox still fills
    // first. Local folders are the exception both ways: the server has never
    // heard of them, so asking it about one is a guaranteed error per cycle.
    for f in all.iter().filter(|f| f.role.is_empty()) {
        if store.folder_is_local(f.id).unwrap_or(false) {
            continue;
        }
        out.push((String::new(), f.path.clone(), f.id));
    }
    out
}

/// Drops Gmail labels that are already Petrel tags from the folder survey.
///
/// On Gmail one server object — the label — backs both of Petrel's ideas, a
/// place and a tag. A tag made here becomes a label there (deliberately: tag
/// names sync, so they survive being seen from any other client), and the
/// next survey would bring that same label back as a *folder*, so the thing
/// you made once appears twice pretending to be two things. A label that is
/// a tag stays a tag. Everywhere else folders and tags are different server
/// objects and a shared name is legitimate, so nothing is dropped.
fn without_tag_labels(
    rows: Vec<(String, Option<String>)>,
    tag_names: &[String],
    is_gmail: bool,
) -> Vec<(String, Option<String>)> {
    if !is_gmail {
        return rows;
    }
    rows.into_iter()
        .filter(|(path, role)| {
            // Role-bearing folders (Sent, Trash, Important…) are never tags.
            role.is_some() || !tag_names.iter().any(|t| t.eq_ignore_ascii_case(path))
        })
        .collect()
}

/// Splits a recipient field the way the composer's chip field does —
/// commas and semicolons — for rendering a draft whose addresses are still
/// one string. A draft may legitimately have none at all.
fn addresses_of(field: &str) -> Vec<String> {
    field
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Pushes one draft to the server's Drafts folder, replacing its previous
/// copy there.
///
/// The draft travels under a Message-ID minted on its first push and kept for
/// life: every later push carries the same one, so the server copy is an edit
/// rather than a sibling — and when ordinary folder sync fetches it back, the
/// dedupe key lands it on the local draft row instead of beside it. The old
/// server copy is deleted only when it is exactly the UID this store
/// recorded; a copy some other client replaced meanwhile is left standing, so
/// a conflicting revision is never silently discarded.
async fn push_draft_to_server(state: &Arc<AppState>, draft_id: i64) -> Result<(), String> {
    let (record, msgid, old_uid, cfg, drafts_path, identity, domain) = {
        let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let record = store.load_draft(draft_id).map_err(|e| e.to_string())?;
        let (msgid, old_uid) = store
            .draft_sync_state(draft_id)
            .map_err(|e| e.to_string())?;
        let Some(cfg) = imap_config_for(&store, account) else {
            // No server to push to is not a failure of the draft.
            return Ok(());
        };
        let drafts_path = store
            .folder_for_role(account, "drafts")
            .ok()
            .flatten()
            .and_then(|fid| store.folder_path(fid).ok().flatten());
        let identity = store.identity(account).ok();
        let domain = cfg
            .user
            .split('@')
            .nth(1)
            .unwrap_or("localhost")
            .to_string();
        let msgid = match msgid {
            Some(m) => m,
            None => {
                let minted = format!(
                    "draft-{:x}.{}@{domain}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0),
                    std::process::id(),
                );
                store
                    .set_draft_msgid(draft_id, &minted)
                    .map_err(|e| e.to_string())?;
                minted
            }
        };
        (record, msgid, old_uid, cfg, drafts_path, identity, domain)
    };
    let Some(drafts_path) = drafts_path else {
        return Ok(());
    };
    let _ = domain;

    let msg = petrel_providers::smtp::Outgoing {
        from_addr: cfg.user.clone(),
        from_name: identity.map(|i| i.display_name).unwrap_or_default(),
        to: addresses_of(&record.to),
        cc: addresses_of(&record.cc),
        subject: record.subject.clone(),
        body_text: record.body.clone(),
        body_html: Some(record.html.clone()).filter(|h| !h.trim().is_empty()),
        in_reply_to: record.envelope.in_reply_to.clone(),
        references: record.envelope.references.clone(),
        // Attachment files stay local until send: a draft's paths may not
        // even exist by the time it is reopened, and pushing megabytes on
        // every autosave is the wrong trade. The text notes nothing; other
        // clients see the words, which is what a draft is.
        attachments: Vec::new(),
    };
    let raw = msg.render_with_id(&msgid);

    petrel_providers::imap::append_message(&cfg, &drafts_path, Some("(\\Draft \\Seen)"), &raw)
        .await
        .map_err(|e| format!("append: {e}"))?;
    let new_uid = petrel_providers::imap::uids_for_message_id(&cfg, &drafts_path, &msgid)
        .await
        .ok()
        .and_then(|hits| hits.last().copied());

    if let Some(old) = old_uid
        && new_uid != Some(old)
    {
        // Only the exact copy this store recorded. Anything else standing at
        // another UID is somebody's revision, and it stays.
        if let Err(e) = petrel_providers::imap::expunge_uid(
            &cfg,
            &drafts_path,
            old,
            state.server_has_uidplus.load(Ordering::Relaxed),
        )
        .await
        {
            log_sync(&format!("old draft copy (uid {old}) not removed: {e}"));
        }
    }
    // Absent (search failed), the next push simply leaves a copy behind
    // rather than deleting blind.
    if let Ok(mut store) = state.store.lock() {
        let _ = store.set_draft_server_uid(draft_id, new_uid);
    }
    log_sync(&format!("draft {draft_id} pushed to {drafts_path}"));
    Ok(())
}

/// Marks the draft dirty and, if it was clean, starts the 30-second clock.
///
/// Saves inside the window coalesce: the sleeping task pushes whatever the
/// draft says when the clock runs out, which is the newest save. Closing the
/// composer pushes immediately through the `push_draft` command instead.
fn schedule_draft_push(state: Arc<AppState>, draft_id: i64) {
    {
        let Ok(mut dirty) = state.draft_dirty.lock() else {
            return;
        };
        if !dirty.insert(draft_id) {
            return; // a task is already sleeping on it
        }
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let still_dirty = state
            .draft_dirty
            .lock()
            .map(|mut d| d.remove(&draft_id))
            .unwrap_or(false);
        if still_dirty && let Err(e) = push_draft_to_server(&state, draft_id).await {
            log_sync(&format!("draft {draft_id} push failed: {e}"));
        }
    });
}

/// Deletes the draft's server copy, if one was recorded — for a draft being
/// discarded, or one that just became a sent message. Reads through the
/// caller's guard, because two of the three callers already hold the lock.
fn drop_server_draft_using(store: &Store, draft_id: i64, uidplus: bool) {
    let Ok((_, Some(uid))) = store.draft_sync_state(draft_id) else {
        return;
    };
    let Some(account) = store.active_account().ok().flatten() else {
        return;
    };
    let Some(cfg) = imap_config_for(store, account) else {
        return;
    };
    let Some(path) = store
        .folder_for_role(account, "drafts")
        .ok()
        .flatten()
        .and_then(|fid| store.folder_path(fid).ok().flatten())
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        // UIDPLUS makes the expunge surgical; without it the fallback path
        // inside expunge_uid does the careful dance. Read fresh per call.
        if let Err(e) = petrel_providers::imap::expunge_uid(&cfg, &path, uid, uidplus).await {
            log_sync(&format!("server draft copy (uid {uid}) not removed: {e}"));
        }
    });
}

/// The lock-acquiring face of `drop_server_draft_using`.
fn spawn_drop_server_draft(state: &Arc<AppState>, draft_id: i64) {
    let uidplus = state.server_has_uidplus.load(Ordering::Relaxed);
    let Ok(store) = state.store.lock() else {
        return;
    };
    drop_server_draft_using(&store, draft_id, uidplus);
}

/// Ingests one fetched message, absorbing a parser panic instead of letting
/// it poison the store lock.
///
/// The sanitizer's rule is "salvage, never judge", but a bug in salvage is a
/// panic — and this callback holds the store lock, so before this fence one
/// hostile message did not cost one message, it cost every pane of the app
/// until relaunch (found the hard way: an HTML-only newsletter with an emoji
/// and a byte-walking tag stripper). The panic is still a bug and still gets
/// fixed; it is just no longer an outage while it waits to be found.
fn ingest_fenced(
    store: &mut Store,
    blobs: &petrel_engine::blob::BlobStore,
    account: i64,
    folder_id: i64,
    uid: u32,
    flags: i64,
    raw: &[u8],
) -> Option<bool> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.ingest_raw(blobs, account, Some(folder_id), Some(uid), raw)
    }));
    match result {
        Ok(Ok(ingested)) => {
            let _ = store.set_message_flags(ingested.message_id, flags);
            // `was_new` is false when the bytes were already here and only a
            // placement was added — how the progress counter avoids counting
            // one message once per folder it appears in.
            Some(ingested.was_new)
        }
        Ok(Err(e)) => {
            log_sync(&format!("ingest uid {uid} failed: {e}"));
            None
        }
        Err(_) => {
            log_sync(&format!(
                "ingest uid {uid} PANICKED — message skipped, bytes not stored; this is a bug worth reporting"
            ));
            None
        }
    }
}

/// One polite stride of history: the next chunk of the first folder whose
/// backfill is not finished.
///
/// The cursor is the lowest UID the folder holds; the floor is the lowest
/// this walk has asked for, so ranges emptied by years of expunges are never
/// asked about twice. Floor 1 is done. Chunks are small and run between
/// cycles, so interactive work — a click, a poll, a send — never waits on
/// history. Returns true when a stride ran, false when every folder is done.
async fn run_backfill_tick(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) -> bool {
    let chunk: u32 = std::env::var("PETREL_BACKFILL_CHUNK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let target = {
        let Ok(store) = state.store.lock() else {
            return false;
        };
        let mut found: Option<(String, i64, u32)> = None;
        for (_role, path, fid) in folders_to_sync_from(&store, account) {
            let held = store.min_uid(fid).ok().flatten();
            let floor = store.backfill_floor(fid).ok().flatten();
            let ceiling = match (floor, held) {
                // Never walked and nothing held: an empty folder is done.
                (None, None) => continue,
                (None, Some(min)) => min,
                (Some(1), _) => continue, // finished
                (Some(f), _) => f,
            };
            if ceiling <= 1 {
                continue;
            }
            found = Some((path, fid, ceiling));
            break;
        }
        match found {
            Some(t) => t,
            None => return false,
        }
    };
    let (path, folder_id, ceiling) = target;
    let first = ceiling.saturating_sub(chunk).max(1);
    let last = ceiling - 1;

    let st = Arc::clone(state);
    let fetched = petrel_providers::imap::fetch_uid_range_each(cfg, &path, first, last, {
        move |uid, flags, raw| {
            let Ok(mut store) = st.store.lock() else {
                return;
            };
            if ingest_fenced(&mut store, &st.blobs, account, folder_id, uid, flags, raw)
                == Some(true)
            {
                st.seeded.fetch_add(1, Ordering::Relaxed);
            }
        }
    })
    .await;

    match fetched {
        Ok(n) => {
            if let Ok(mut store) = state.store.lock() {
                let _ = store.set_backfill_floor(folder_id, first);
            }
            if n > 0 {
                log_sync(&format!(
                    "backfill {path}: {n} message(s), down to uid {first}"
                ));
            }
            true
        }
        Err(e) => {
            log_sync(&format!("backfill {path} failed: {e}"));
            // Failed is not finished: the same stride retries next tick.
            true
        }
    }
}

/// One incremental Gmail label sweep: where every message lives, which are
/// starred, and — for labels that are Petrel tags — who carries them. With
/// CONDSTORE this costs one round trip when nothing changed, which is why it
/// can run every cycle rather than once at startup: a label applied in
/// Gmail's web UI shows up here within a poll interval.
async fn run_label_sweep(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) {
    let since: Option<u64> = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.settings().ok())
        .and_then(|s| s.get("gmail_labels_modseq").and_then(|v| v.parse().ok()));
    let bound: u32 = std::env::var("PETREL_LABEL_SWEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    match petrel_providers::imap::sweep_gmail_labels(cfg, "[Gmail]/All Mail", bound, since).await {
        Ok(sweep) => {
            let filed = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.apply_gmail_labels(account, &sweep.labels).ok())
                .unwrap_or(0);
            if !sweep.labels.is_empty() {
                log_sync(&format!(
                    "labels: {} reported, {filed} refiled",
                    sweep.labels.len()
                ));
            }
            if let (Some(m), Ok(store)) = (sweep.modseq, state.store.lock()) {
                let _ = store.set_setting("gmail_labels_modseq", &m.to_string());
            }
        }
        // Not fatal: without it, filing falls back to the folder each
        // message arrived from, which is what it was before.
        Err(e) => log_sync(&format!("label sweep failed: {e}")),
    }
}

/// One sync cycle for one account: every folder, one connection.
///
/// The shape of the whole optimisation. A cycle logs in once, asks one
/// STATUS line per folder, and only selects and fetches the folders where
/// something actually moved — so a quiet cycle over a hundred folders is a
/// hundred cheap lines on one connection, and a relaunch re-downloads
/// nothing it already holds: a folder with a watermark is only ever asked
/// for what is above it. Flag changes made elsewhere ride along via
/// CONDSTORE where the server has it. Returns (new messages, failures).
async fn run_sync_cycle(
    state: &Arc<AppState>,
    account: i64,
    cfg: &ImapConfig,
    verbose: bool,
) -> (usize, usize) {
    let targets = folders_to_sync(state, account);
    if targets.is_empty() {
        return (0, 0);
    }
    let window: u32 = std::env::var("PETREL_SYNC_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let passes: Vec<petrel_providers::imap::FolderPass> = {
        let Ok(store) = state.store.lock() else {
            return (0, 0);
        };
        targets
            .iter()
            .map(|(_, path, fid)| petrel_providers::imap::FolderPass {
                path: path.clone(),
                since_uid: store.max_uid(*fid).ok().flatten().unwrap_or(0),
                expected_validity: store.folder_validity(*fid).ok().flatten(),
                since_uidnext: store.folder_uidnext(*fid).ok().flatten(),
                since_modseq: store.folder_modseq(*fid).ok().flatten(),
                seed_window: window,
            })
            .collect()
    };

    let mut fresh = 0usize;
    // Messages that genuinely *arrived* — a watermark fetch into the inbox,
    // not a seed window and not backfill. These are what filter rules run on:
    // "on arrival" must never mean "on downloading five years of archive".
    let mut arrivals: Vec<i64> = Vec::new();
    let inbox_folder: Option<i64> = {
        let Ok(store) = state.store.lock() else {
            return (0, 0);
        };
        store.folder_for_role(account, "inbox").ok().flatten()
    };
    let outcomes = {
        let st = Arc::clone(state);
        let ids: Vec<i64> = targets.iter().map(|(_, _, id)| *id).collect();
        let arriving: Vec<bool> = passes
            .iter()
            .zip(&ids)
            .map(|(p, fid)| p.since_uid > 0 && Some(*fid) == inbox_folder)
            .collect();
        let arrivals = &mut arrivals;
        petrel_providers::imap::sync_pass(cfg, &passes, |index, uid, flags, raw| {
            let Ok(mut store) = st.store.lock() else {
                return;
            };
            if ingest_fenced(&mut store, &st.blobs, account, ids[index], uid, flags, raw)
                == Some(true)
            {
                fresh += 1;
                st.seeded.fetch_add(1, Ordering::Relaxed);
                if arriving[index] {
                    // The id of what just landed, by its placement.
                    if let Ok(Some(mid)) = store.message_id_at(ids[index], uid) {
                        arrivals.push(mid);
                    }
                }
            }
        })
        .await
    };
    let outcomes = match outcomes {
        Ok(o) => o,
        Err(e) => {
            log_sync(&format!("sync cycle failed before any folder: {e}"));
            return (fresh, targets.len());
        }
    };

    use petrel_providers::imap::PassOutcome;
    let mut failures = 0usize;
    let mut server_total = 0usize;
    for (((_, path, folder_id), pass), outcome) in targets.iter().zip(&passes).zip(&outcomes) {
        match outcome {
            PassOutcome::Unchanged {
                uid_validity,
                highest_modseq,
                uid_next,
                total,
            } => {
                server_total += *total as usize;
                if let Ok(mut store) = state.store.lock() {
                    if pass.expected_validity.is_none() {
                        let _ = store.set_folder_validity(*folder_id, *uid_validity);
                    }
                    // A quiet folder with no baselines adopts them, so the
                    // next change is a diff instead of a mystery.
                    if pass.since_modseq.is_none()
                        && let Some(m) = highest_modseq
                    {
                        let _ = store.set_folder_modseq(*folder_id, *m);
                    }
                    if pass.since_uidnext.is_none()
                        && let Some(n) = uid_next
                    {
                        let _ = store.set_folder_uidnext(*folder_id, *n);
                    }
                }
            }
            PassOutcome::Fetched {
                fetched,
                uid_validity,
                highest_modseq,
                uid_next,
                flag_updates,
                total,
            } => {
                server_total += *total as usize;
                let mut reflagged = 0usize;
                if let Ok(mut store) = state.store.lock() {
                    if pass.expected_validity.is_none() {
                        let _ = store.set_folder_validity(*folder_id, *uid_validity);
                    }
                    if let Some(m) = highest_modseq {
                        let _ = store.set_folder_modseq(*folder_id, *m);
                    }
                    if let Some(n) = uid_next {
                        let _ = store.set_folder_uidnext(*folder_id, *n);
                    }
                    for (uid, flags) in flag_updates {
                        if store
                            .set_flags_by_uid(*folder_id, *uid, *flags)
                            .unwrap_or(false)
                        {
                            reflagged += 1;
                        }
                    }
                }
                if verbose || *fetched > 0 || reflagged > 0 {
                    log_sync(&format!(
                        "{path}: {fetched} fetched, {reflagged} flag update(s)"
                    ));
                }
            }
            PassOutcome::ValidityChanged { now } => {
                log_sync(&format!(
                    "{path}: UIDVALIDITY reset ({:?} -> {now:?}); re-mapping",
                    pass.expected_validity
                ));
                if let Ok(mut store) = state.store.lock() {
                    // The modseq domain does not survive a renumbering.
                    let _ = store.clear_folder_modseq(*folder_id);
                }
                match recover_folder(state, account, cfg, path, *folder_id).await {
                    Ok(_) => {}
                    Err(e) => {
                        log_sync(&format!("{path}: recovery failed: {e}"));
                        failures += 1;
                    }
                }
            }
            PassOutcome::Failed { detail } => {
                if verbose {
                    log_sync(&format!("{path}: FAILED: {detail}"));
                }
                failures += 1;
            }
        }
    }
    state.server_total.store(server_total, Ordering::Relaxed);
    if !arrivals.is_empty() {
        apply_rules_to(state, account, &arrivals);
    }
    (fresh, failures)
}

/// Runs the account's filter rules over newly-arrived messages.
///
/// Every enabled rule that matches contributes, in the user's order, and
/// each action goes through the ordinary triage path — locally at once,
/// queued to the server like a hand-made change, drained promptly.
fn apply_rules_to(state: &Arc<AppState>, account: i64, arrivals: &[i64]) {
    use petrel_engine::actions::ActionKind;
    let Ok(store) = state.store.lock() else {
        return;
    };
    let Ok(rules) = store.rules_for_account(account) else {
        return;
    };
    if rules.iter().all(|r| !r.enabled || r.conditions.is_empty()) {
        return;
    }
    let Ok(policy) = store.placement_policy(account) else {
        return;
    };
    let mut applied = 0usize;
    for &message_id in arrivals {
        let Ok(Some(hash)) = store.blob_hash_for(message_id) else {
            continue;
        };
        let Ok(raw) = state.blobs.read(&hash) else {
            continue;
        };
        let Some(parsed) = petrel_mime::parse_message(&raw) else {
            continue;
        };
        let to = parsed
            .to
            .iter()
            .map(|(_, a)| a.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let envelope = petrel_engine::rules::Envelope::new(
            &format!(
                "{} {}",
                parsed.from_display.as_deref().unwrap_or(""),
                parsed.from_addr.as_deref().unwrap_or("")
            ),
            &to,
            parsed.subject.as_deref().unwrap_or(""),
            parsed.list_id.as_deref().unwrap_or(""),
        );
        let Ok(Some(thread)) = store.thread_of(message_id) else {
            continue;
        };
        for rule in &rules {
            if !petrel_engine::rules::matches(rule, &envelope) {
                continue;
            }
            let a = &rule.actions;
            let mut acts: Vec<(ActionKind, Option<i64>)> = Vec::new();
            if let Some(folder) = a.move_to {
                acts.push((ActionKind::Move, Some(folder)));
            }
            if a.skip_inbox {
                acts.push((ActionKind::Archive, None));
            }
            if let Some(tag) = a.tag {
                acts.push((ActionKind::Tag, Some(tag)));
            }
            if a.mark_read {
                acts.push((ActionKind::MarkRead, None));
            }
            for (kind, target) in acts {
                if let Err(e) = store.apply_thread_action(account, thread, kind, target, policy) {
                    log_sync(&format!("rule \"{}\": {e}", rule.name));
                } else {
                    applied += 1;
                }
            }
        }
    }
    if applied > 0 {
        log_sync(&format!("rules: {applied} action(s) applied on arrival"));
        state.drain_signal.notify_one();
    }
}

/// Mends one folder after the server renumbered it (UIDVALIDITY reset).
///
/// The order is the safety: quarantine and re-map by Message-ID first (the
/// store's transaction), then download what could not be matched, and record
/// the new validity *last* — so a crash anywhere in between leaves the old
/// value in place and the next pass simply runs recovery again. Message rows
/// and blobs are never deleted; the worst case is re-downloading, never data.
async fn recover_folder(
    state: &Arc<AppState>,
    account: i64,
    cfg: &ImapConfig,
    name: &str,
    folder_id: i64,
) -> Result<usize, String> {
    let depth: u32 = std::env::var("PETREL_SYNC_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let map = petrel_providers::imap::fetch_id_map(cfg, name, depth)
        .await
        .map_err(|e| format!("id map: {e}"))?;
    let outcome = {
        let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store
            .remap_folder_after_reset(folder_id, &map.entries, map.complete)
            .map_err(|e| format!("remap: {e}"))?
    };
    let mut refetched = 0usize;
    if !outcome.to_fetch.is_empty() {
        let st = Arc::clone(state);
        refetched = petrel_providers::imap::fetch_uids_each(
            cfg,
            name,
            &outcome.to_fetch,
            |uid, flags, raw| {
                let Ok(mut store) = st.store.lock() else {
                    return;
                };
                let _ = ingest_fenced(&mut store, &st.blobs, account, folder_id, uid, flags, raw);
            },
        )
        .await
        .map_err(|e| format!("refetch: {e}"))?;
    }
    {
        let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store
            .set_folder_validity(folder_id, map.uid_validity)
            .map_err(|e| format!("record validity: {e}"))?;
    }
    log_sync(&format!(
        "{name}: re-mapped {} placement(s), re-downloaded {refetched}, dropped {}",
        outcome.rematched, outcome.dropped
    ));
    Ok(refetched)
}

fn spawn_real_sync(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
    spawn_drain_worker(Arc::clone(&state), account, cfg.clone());
    spawn_outbox_clock(Arc::clone(&state), account);
    tauri::async_runtime::spawn(async move {
        *state.source.lock().unwrap() = format!("syncing {}…", cfg.host);

        // Mail already held was indexed by whatever the extraction did then.
        // When that improves, the improvement has to be applied backwards or it
        // only ever reaches mail that has not arrived yet.
        {
            if let Ok(mut store) = state.store.lock() {
                match store.reindex_bodies(&state.blobs) {
                    Ok(0) => {}
                    Ok(n) => log_sync(&format!(
                        "re-indexed {n} message(s) after an extraction change"
                    )),
                    Err(e) => log_sync(&format!("re-index failed: {e}")),
                }
            }
        }

        let mut has_move = false;
        let mut has_idle = false;
        let mut has_uidplus = false;
        // Whether this account's folders are labels, which decides whether the
        // label sweep below has anything to ask for.
        let mut looks_like_gmail = false;
        // Folders first. Without them every message ingests with no placement,
        // so the rail's views have nothing to filter on and archiving has
        // nowhere to put anything — which is how a sync can look like it worked
        // while leaving the app unable to file a single message.
        match petrel_providers::imap::probe(&cfg, 0).await {
            Ok(report) => {
                has_move = report.greeting_capabilities.move_;
                has_idle = report.greeting_capabilities.idle;
                has_uidplus = report.greeting_capabilities.uidplus;
                state.server_has_move.store(has_move, Ordering::Relaxed);
                state
                    .server_has_uidplus
                    .store(has_uidplus, Ordering::Relaxed);
                log_sync(&format!(
                    "probe ok: {} folder(s), MOVE={has_move}, IDLE={has_idle}, UIDPLUS={has_uidplus}",
                    report.folders.len(),
                ));
                let rows: Vec<(String, Option<String>)> = report
                    .folders
                    .iter()
                    // \Noselect containers ([Gmail] itself) are hierarchy,
                    // not mailboxes: nothing to list, nothing to sync.
                    .filter(|f| petrel_providers::imap::selectable(f))
                    .map(|f| {
                        (
                            f.name.clone(),
                            petrel_providers::imap::special_use_role(f).map(|r| r.to_string()),
                        )
                    })
                    .collect();
                // Gmail is the provider whose folders are labels, and the only
                // one we can identify from what it advertises before any mail
                // arrives. Recording it here is what makes archiving keep the
                // user's other labels instead of clearing them.
                looks_like_gmail = cfg.host.contains("gmail")
                    || report.folders.iter().any(|f| f.name.starts_with("[Gmail]"));
                state
                    .server_is_gmail
                    .store(looks_like_gmail, Ordering::Relaxed);
                if let Ok(mut store) = state.store.lock() {
                    let tag_names: Vec<String> = store
                        .tags_for_account(account)
                        .map(|ts| ts.into_iter().map(|t| t.name).collect())
                        .unwrap_or_default();
                    let rows = without_tag_labels(rows, &tag_names, looks_like_gmail);
                    match store.sync_folders(account, &rows) {
                        Ok(n) => log_sync(&format!("{n} folder(s) stored")),
                        Err(e) => log_sync(&format!("folder sync failed: {e}")),
                    }
                    if looks_like_gmail {
                        let _ = store.set_account_kind(account, "gmail");
                    }
                }
            }
            Err(e) => {
                log_sync(&format!("folder discovery FAILED: {e}"));
                *state.sync_error.lock().unwrap() = Some(friendly_sync_error(&format!("{e}")));
            }
        }

        // Deliver before reading back. Draining first means the server's answer
        // already includes what the user did, so the fetch below confirms local
        // state instead of contradicting it — and anything still queued is
        // protected from being overwritten by the pending checks in the store.
        drain_actions(
            Arc::clone(&state),
            account,
            cfg.clone(),
            has_move,
            has_uidplus,
            state.server_is_gmail.load(Ordering::Relaxed),
        )
        .await;
        // A message due while the app was closed goes out now, rather than
        // waiting for whatever next wakes the worker.
        send_due(Arc::clone(&state), account).await;

        // One connection, one STATUS line per folder, fetch only what moved.
        // A relaunch over a warm store downloads nothing it already holds.
        let (fresh, failures) = run_sync_cycle(&state, account, &cfg, true).await;
        let targets = folders_to_sync(&state, account);
        if failures > 0 {
            log_sync(&format!("{failures} folder(s) could not be synced"));
        }
        if !targets.is_empty() && failures >= targets.len() {
            let msg = "no folder could be synced";
            log_sync(msg);
            *state.sync_error.lock().unwrap() = Some(friendly_sync_error(msg));
            *state.source.lock().unwrap() = "sync failed".into();
        } else {
            let held = state.seeded.load(Ordering::Relaxed);
            log_sync(&format!(
                "first pass done: {fresh} new, {held} held locally"
            ));
            *state.source.lock().unwrap() = format!("{} · {held} message(s) held", cfg.user);
        }
        // Where Gmail actually keeps each message.
        //
        // After the bodies rather than before: this decides filing, and filing
        // an empty mailbox helps nobody. Over plain IMAP a message is only ever
        // in the mailbox it was fetched from, so archived — not carrying the
        // Inbox label — is not something the protocol can express.
        //
        // Bounded on the first pass and incremental after it. A full sweep is
        // seconds at a thousand messages and minutes at a hundred thousand,
        // but with CONDSTORE every sweep after the first asks only for what
        // changed, which is usually nothing and costs one round trip.
        if looks_like_gmail {
            run_label_sweep(&state, account, &cfg).await;
        }

        state.seeding.store(false, Ordering::Relaxed);

        // From here on, poll. The first pass took a window of recent mail;
        // every pass after it asks only for UIDs above the highest we hold, so
        // a poll costs one round trip when nothing has arrived.
        //
        // Polling rather than IDLE for now: IDLE needs a connection held open
        // and re-issued every 29 minutes, and getting that wrong fails in the
        // worst way — silently, by simply never delivering anything. A poll is
        // duller and its failure mode is visible.
        let every = std::time::Duration::from_secs(
            std::env::var("PETREL_POLL_SECONDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|s| *s >= 15)
                .unwrap_or(120),
        );
        // RFC 2177 puts the ceiling at 29 minutes; 20 leaves room for a server
        // that is stricter than the standard without making reconnects frequent.
        let idle_ceiling = std::time::Duration::from_secs(20 * 60);
        log_sync(&format!(
            "watching for new mail via {}",
            if has_idle { "IDLE" } else { "poll" }
        ));

        loop {
            if has_idle {
                // Held open until the server speaks, so mail lands immediately
                // rather than on the next tick. A failure here drops through to
                // the poll below rather than ending the loop: losing push is a
                // reason to check more slowly, not to stop checking.
                match petrel_providers::imap::idle_once(&cfg, "INBOX", idle_ceiling).await {
                    Ok(_) => {}
                    Err(e) => {
                        log_sync(&format!("idle failed, falling back to poll: {e}"));
                        tokio::time::sleep(every).await;
                    }
                }
            } else {
                tokio::time::sleep(every).await;
            }

            // Deliver first, so the fetch that follows confirms local state
            // rather than contradicting it — the same ordering as startup.
            drain_actions(
                Arc::clone(&state),
                account,
                cfg.clone(),
                has_move,
                has_uidplus,
                state.server_is_gmail.load(Ordering::Relaxed),
            )
            .await;
            send_due(Arc::clone(&state), account).await;

            // One connection for the whole account, STATUS-gated per folder:
            // a quiet cycle costs a line per folder, not a login per folder.
            let (fresh, failures) = run_sync_cycle(&state, account, &cfg, false).await;
            if state.server_is_gmail.load(Ordering::Relaxed) {
                // One round trip when nothing changed; live labels when it did.
                run_label_sweep(&state, account, &cfg).await;
            }
            // History fills in behind the present: a few polite strides per
            // quiet cycle, none at all while new mail is arriving. Each
            // stride is small, so the next IDLE wake or click never waits
            // long behind it — and the cursor means a restart loses nothing.
            if fresh == 0 {
                for _ in 0..5 {
                    if !run_backfill_tick(&state, account, &cfg).await {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
            let trouble: Option<String> = if failures > 0 {
                Some(format!("{failures} folder(s) failed"))
            } else {
                None
            };
            if fresh > 0 {
                log_sync(&format!("poll: {fresh} new message(s)"));
                // The list watches this count, so bumping it is what makes
                // new mail appear without the user doing anything.
            }
            // Only a pass that both found nothing and hit nothing clears the
            // banner: a poll that failed halfway is not proof that sync is well.
            if trouble.is_none() {
                *state.sync_error.lock().unwrap() = None;
            }
        }
    });
}

#[tauri::command]
fn list_messages(
    offset: u32,
    limit: u32,
    state: State<Arc<AppState>>,
) -> Result<Vec<Listing>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        // A virtualized list wants a real window, not 100 rows. The proper fix is
        // fetching windows as the user scrolls; this cap is the interim.
        .list_recent(offset, limit.min(2000))
        .map_err(|e| e.to_string())
}

/// The list shows conversations, not messages — the count chip is the thread
/// size (docs 06). Flags are rolled up across the thread by the engine.
#[tauri::command]
fn list_threads(
    view: Option<String>,
    offset: u32,
    limit: u32,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadListing>, String> {
    // The rail key is parsed by the engine, which owns the mapping from a view
    // to a query. An absent view means the inbox.
    let view = ListView::parse(view.as_deref().unwrap_or("inbox"));
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .list_threads(&view, offset, limit.min(2000))
        .map_err(|e| e.to_string())
}

/// Opens a link from a message in the user's browser.
///
/// Mail is the most-phished medium there is, so the scheme is checked here
/// rather than trusted from the frame. Only the two web schemes are handed to
/// the system: `file:` would open local content, `javascript:` is an execution
/// vector, and the custom schemes registered by other applications on the
/// machine are a large and unaudited surface reachable from any sender.
///
/// `mailto:` is deliberately absent — the app answers that itself by opening a
/// composer, rather than handing a mail link to some other mail program.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http and https links can be opened".into());
    }
    // Passed as a single argument to the platform's opener, never through a
    // shell, so nothing in the URL can be read as a command.
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler").arg(&url);
        c
    };
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// One conversation by id, for a window that was opened onto it.
///
/// Separate from `list_threads` because a popped-out window has an id and no
/// view: it cannot say which mailbox to look in, and guessing is what made it
/// claim that starred and archived conversations no longer existed.
#[tauri::command]
fn thread_by_id(
    thread_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<ThreadListing>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.thread_by_id(thread_id).map_err(|e| e.to_string())
}

/// Tags for the rail. Comes from the account, not from whatever rows happen to
/// be loaded — a tag with no conversation in the current page still exists.
#[tauri::command]
fn list_tags(state: State<Arc<AppState>>) -> Result<Vec<TagSummary>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.tags_for_account(account).map_err(|e| e.to_string())
}

/// The numbers beside the rail's mailboxes.
///
/// The mode comes from the caller rather than from stored settings because the
/// setting lives in the renderer with the rest of them, and a second copy in
/// the engine is a second thing to keep in step.
#[tauri::command]
fn view_counts(mode: String, state: State<Arc<AppState>>) -> Result<Vec<(String, i64)>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .view_counts(petrel_engine::store::CountMode::parse(&mode))
        .map_err(|e| e.to_string())
}

/// Who sent this, and whether their remote content is already allowed.
///
/// The reader asks so its banner can offer the right thing: the sender's
/// address to name in "always show images from …", and whether to bother
/// offering at all.
#[derive(serde::Serialize)]
struct RemoteStatus {
    from_addr: String,
    allowed: bool,
    /// True when it is allowed because the user has written to them, rather
    /// than because they were trusted by hand. The two are worth telling apart:
    /// one is a decision to revisit in settings, the other is not a decision
    /// at all and there is nothing in the list to find.
    because_written_to: bool,
}

#[tauri::command]
fn remote_status(message_id: i64, state: State<Arc<AppState>>) -> Result<RemoteStatus, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let from = store
        .message_sender(message_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(RemoteStatus {
            from_addr: from,
            allowed: false,
            because_written_to: false,
        });
    };
    let trusted = store
        .sender_trusted(account, &from)
        .map_err(|e| e.to_string())?;
    let written = store
        .has_written_to(account, &from)
        .map_err(|e| e.to_string())?;
    Ok(RemoteStatus {
        from_addr: from,
        allowed: trusted || written,
        because_written_to: written && !trusted,
    })
}

/// Shows this one message's remote content, for as long as the app is running.
#[tauri::command]
fn show_remote_once(message_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    state
        .shown_once
        .lock()
        .map_err(|_| "lock poisoned")?
        .insert(message_id);
    Ok(())
}

/// Trusts this message's sender from now on.
#[tauri::command]
fn trust_sender(message_id: i64, state: State<Arc<AppState>>) -> Result<String, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Err("no account".into());
    };
    let from = store
        .message_sender(message_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if from.is_empty() {
        return Err("this message has no sender to trust".into());
    }
    store
        .trust_sender(account, &from, now_ms())
        .map_err(|e| e.to_string())?;
    Ok(from)
}

#[tauri::command]
fn trusted_senders(state: State<Arc<AppState>>) -> Result<Vec<String>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.trusted_senders(account).map_err(|e| e.to_string())
}

#[tauri::command]
fn untrust_sender(addr: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(());
    };
    store
        .untrust_sender(account, &addr)
        .map_err(|e| e.to_string())
}

/// The messages of one conversation, for the reading pane.
#[tauri::command]
fn thread_detail(
    thread_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadMessage>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.thread_detail(thread_id).map_err(|e| e.to_string())
}

/// Applies a triage action locally and queues it. Returns the receipt the UI
/// needs to offer undo, so the frontend holds no state of its own about what it
/// just did.
#[tauri::command]
fn triage(
    thread_id: i64,
    kind: ActionKind,
    target: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<ActionReceipt, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    // The provider's placement model, not a per-call guess: on Gmail an
    // archive removes one label, on a classic server it replaces the folder.
    let policy = store.placement_policy(account).map_err(|e| e.to_string())?;
    let receipt = store
        .apply_thread_action(account, thread_id, kind, target, policy)
        .map_err(|e| e.to_string())?;
    // Local change done; ask for it to be delivered. The lock is released as
    // this returns, so the drain is never waiting on the caller.
    state.drain_signal.notify_one();
    Ok(receipt)
}

/// Sends a message, then files a copy in Sent.
///
/// Called only after the undo window has lapsed: nothing reaches the server
/// while the countdown is running, which is what makes undo a cancel rather
/// than a recall. By the time this runs, the user has committed.
///
/// The two halves are reported separately on purpose. A send that succeeded and
/// an append that failed is a delivered message with no local record of it —
/// annoying, but not a failure to send, and telling someone their mail did not
/// go when it did is worse than telling them it did not get filed.
#[tauri::command]
async fn send_message(
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    body: String,
    html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    deliver(
        state.inner(),
        to,
        cc,
        subject,
        body,
        html,
        in_reply_to,
        references,
        attachments,
    )
    .await
}

/// Builds and sends one message, then files a copy in Sent.
///
/// Shared by the composer and the scheduled-send worker so there is one
/// definition of what sending means — two would drift, and the half that
/// drifted would be the one nobody watches.
#[allow(clippy::too_many_arguments)]
/// What one attempt to send came to.
///
/// Carries the transport's verdict rather than collapsing it to an error
/// string, because the four verdicts call for four different responses: done,
/// try again, stop and tell the user, or — the one that matters — *find out*.
struct Attempt {
    message_id: String,
    outcome: petrel_engine::outbox::AttemptOutcome,
    /// The server's words, for the row to show when it stopped.
    detail: String,
}

/// Sends once and reports what happened. Never retries, never files a copy in
/// Sent on an uncertain outcome — that is the caller's decision to make, with
/// the state machine's help.
async fn attempt(
    state: &Arc<AppState>,
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    body: String,
    html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<String>,
) -> Result<Attempt, String> {
    use petrel_engine::outbox::AttemptOutcome;
    use petrel_providers::smtp::{Attachment, Outgoing, SendResult, SmtpConfig, send_tls};

    let account_id = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store.active_account().ok().flatten()
    };
    let cfg = account_id
        .and_then(|a| imap_config(state, a))
        .ok_or("no account is configured")?;
    // The SMTP host the account was set up with. Derived from the IMAP host
    // only for the environment-driven account, which has no record of its own.
    let smtp = account_id
        .and_then(|a| {
            state
                .store
                .lock()
                .ok()
                .and_then(|st| smtp_config_for(&st, a))
        })
        .unwrap_or_else(|| SmtpConfig::for_imap_host(&cfg.host, &cfg.user, &cfg.pass));
    let domain = cfg
        .user
        .split('@')
        .nth(1)
        .unwrap_or("localhost")
        .to_string();

    let identity = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store
            .active_account()
            .ok()
            .flatten()
            .and_then(|a| store.identity(a).ok())
    };
    // Read here rather than shuttled through the bridge. A 20MB file becomes a
    // 27MB JSON string on the way across, held twice in memory and slow enough
    // to notice; the path costs nothing and the file is read once.
    let mut files = Vec::new();
    for path in &attachments {
        let p = std::path::Path::new(path);
        let bytes = std::fs::read(p).map_err(|e| format!("Could not read {path}: {e}"))?;
        files.push(Attachment {
            filename: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into()),
            content_type: guess_content_type(p),
            bytes,
        });
    }

    let msg = Outgoing {
        from_addr: cfg.user.clone(),
        from_name: identity.map(|i| i.display_name).unwrap_or_default(),
        to,
        cc,
        subject,
        body_text: body,
        body_html: html.filter(|h| !h.trim().is_empty()),
        in_reply_to,
        references,
        attachments: files,
    };
    if msg.recipients().is_empty() {
        return Err("a message needs at least one recipient".into());
    }
    let (message_id, raw) = msg.render(&domain);

    let (outcome, detail) = match send_tls(&smtp, &msg, &raw).await {
        SendResult::Committed { response } => (AttemptOutcome::Accepted, response),
        SendResult::RejectedPermanently { response } => {
            log_sync(&format!("send rejected: {response}"));
            (AttemptOutcome::RejectedPermanently, response)
        }
        SendResult::FailedBeforeCommit { stage, detail } => {
            log_sync(&format!("send failed at {stage}: {detail}"));
            (
                AttemptOutcome::FailedBeforeCommit,
                format!("{stage}: {detail}"),
            )
        }
        SendResult::UnknownAfterTransmit { detail } => {
            // Spike S5's case: the body went, the acknowledgement did not. The
            // message may well have been delivered, so a retry could duplicate
            // it. Reported as exactly that, for the caller to resolve by
            // looking rather than by guessing.
            log_sync(&format!(
                "send outcome unknown: {detail} (message-id {message_id})"
            ));
            (AttemptOutcome::UnknownAfterTransmit, detail)
        }
    };
    if outcome != AttemptOutcome::Accepted {
        return Ok(Attempt {
            message_id,
            outcome,
            detail,
        });
    }

    // Filed second, and separately: a failure here has not lost the message.
    let sent_path = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let account = store
            .active_account()
            .map_err(|e| e.to_string())?
            .ok_or("no account")?;
        store
            .folder_for_role(account, "sent")
            .ok()
            .flatten()
            .and_then(|fid| store.folder_path(fid).ok().flatten())
    };
    if let Some(path) = sent_path {
        if let Err(e) =
            petrel_providers::imap::append_message(&cfg, &path, Some("(\\Seen)"), &raw).await
        {
            log_sync(&format!("sent, but could not file a copy in {path}: {e}"));
        }
    }
    log_sync(&format!("sent {message_id}"));
    Ok(Attempt {
        message_id,
        outcome,
        detail,
    })
}

/// Sends now, for the composer's Send button: one attempt, reported as a plain
/// success or a sentence the toast can show. The outbox's retry and
/// reconciliation live in `send_due`, which drives `attempt` directly.
async fn deliver(
    state: &Arc<AppState>,
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    body: String,
    html: Option<String>,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<String>,
) -> Result<String, String> {
    use petrel_engine::outbox::AttemptOutcome;
    let a = attempt(
        state,
        to,
        cc,
        subject,
        body,
        html,
        in_reply_to,
        references,
        attachments,
    )
    .await?;
    match a.outcome {
        AttemptOutcome::Accepted => Ok(a.message_id),
        AttemptOutcome::RejectedPermanently => {
            Err(format!("The server refused the message: {}", a.detail))
        }
        AttemptOutcome::FailedBeforeCommit => Err(format!("Could not send ({})", a.detail)),
        AttemptOutcome::UnknownAfterTransmit => Err(
            "The message may have been sent — the connection dropped before the server \
             confirmed. Check your Sent folder before sending it again."
                .into(),
        ),
    }
}

/// Opens a saved draft in a window of its own.
///
/// The window loads the app with `?compose=<id>`, which renders the composer
/// alone rather than a second copy of the whole client. A pop-out exists so a
/// long message can have the screen; giving it another rail and message list
/// would defeat the point and cost a second sync loop.
///
/// The draft must already be saved — the id is the only thing the new window
/// gets, and it is also what stops the two windows from being separate
/// unsaved copies of the same message.
#[tauri::command]
fn popout_compose(draft_id: i64, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let label = format!("compose-{draft_id}");
    // Already open: focus it rather than making a second window onto the same
    // draft, which would leave two editors racing to save over each other.
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?compose={draft_id}").into()),
    )
    .title("Petrel")
    .inner_size(720.0, 620.0)
    .min_inner_size(420.0, 360.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Addresses to offer while a recipient is being typed.
#[tauri::command]
fn complete_addresses(
    prefix: String,
    state: State<Arc<AppState>>,
) -> Result<Vec<petrel_engine::store::Correspondent>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store
        .complete_addresses(account, &prefix, now_ms(), 8)
        .map_err(|e| e.to_string())
}

/// Opens one conversation in a window of its own.
///
/// The same bundle with a query parameter, as the popped-out composer is: a
/// second rail, list and sync loop would cost real memory and a second poll
/// against the mail server to show one thread.
#[tauri::command]
fn popout_message(thread_id: i64, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let label = format!("message-{thread_id}");
    // Already open: focus it. A second window onto the same conversation is
    // never what was meant, and both would drift as it is triaged.
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?message={thread_id}").into()),
    )
    .title("Petrel")
    .inner_size(780.0, 700.0)
    .min_inner_size(420.0, 360.0)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Marks a draft to go later, or pulls it back.
#[tauri::command]
fn schedule_send(
    draft_id: i64,
    at_ms: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .schedule_send(draft_id, at_ms)
        .map_err(|e| e.to_string())?;
    // Wake the worker: a message due in the past should not wait for the next
    // poll just because it was scheduled after the fact.
    state.drain_signal.notify_one();
    Ok(())
}

/// Sends anything in the outbox whose time has come.
///
/// Runs on the same signal as the action drain, so a scheduled send goes out
/// promptly rather than on whatever the sync cadence happens to be. Failures
/// leave the message in the outbox: a send that did not happen is not one to
/// forget about, and the next pass will try again.
/// The retry ladder, in milliseconds: 30s, 2m, 8m, 30m, then hourly.
///
/// Exponential and capped, as the spec asks, and starting short because the
/// common failure is a connection that comes back in seconds. Past the cap
/// there is no giving up: a message waiting for a network that is genuinely
/// away for a day should still go when it returns, not be abandoned because
/// it waited too long.
fn retry_delay_ms(attempts: i64) -> i64 {
    const LADDER: [i64; 4] = [30_000, 120_000, 480_000, 1_800_000];
    LADDER
        .get(attempts.max(1) as usize - 1)
        .copied()
        .unwrap_or(3_600_000)
}

/// Looks in Sent for a message whose send outcome the transport could not
/// report.
///
/// This is the whole of the ambiguous-outcome rule's second half. SMTP has no
/// way to ask "did you get that?", so the answer has to come from evidence:
/// the copy a delivering server places in Sent. Found means it went. Searched
/// and absent means it did not, and a retry is safe. Unable to search — which
/// is the likely case straight after a dropped connection — means nobody
/// knows, and that is the one answer that must reach a person unchanged.
async fn sent_folder_evidence(
    state: &Arc<AppState>,
    cfg: &petrel_providers::imap::ImapConfig,
    account: i64,
    message_id: &str,
) -> petrel_engine::outbox::ServerEvidence {
    use petrel_engine::outbox::ServerEvidence;
    let sent = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.folder_for_role(account, "sent").ok().flatten())
        .and_then(|fid| {
            state
                .store
                .lock()
                .ok()
                .and_then(|s| s.folder_path(fid).ok().flatten())
        });
    let Some(path) = sent else {
        // No Sent folder known for this account: there is nowhere to look.
        return ServerEvidence::Indeterminate;
    };
    match petrel_providers::imap::find_message_id(cfg, &path, message_id).await {
        Ok(uids) if !uids.is_empty() => ServerEvidence::Found,
        Ok(_) => ServerEvidence::Absent,
        Err(e) => {
            log_sync(&format!("could not check Sent for {message_id}: {e}"));
            ServerEvidence::Indeterminate
        }
    }
}

/// Sends whatever is due, and records where each message ended up.
///
/// This is the outbox's state machine in motion. Each attempt's outcome goes
/// through `reconcile`, which is the only place that decides what a message's
/// state becomes — and for the uncertain case it decides by *looking*, in the
/// Sent folder, rather than by guessing. A message the engine cannot prove
/// either way is held for a person and is never picked up here again.
async fn send_due(state: Arc<AppState>, account: i64) {
    use petrel_engine::outbox::{AttemptOutcome, SendState, ServerEvidence, reconcile};

    let due = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        store.due_sends(account, now_ms()).unwrap_or_default()
    };
    if due.is_empty() {
        return;
    }
    log_sync(&format!("{} queued message(s) due", due.len()));
    let Some(cfg) = imap_config(&state, account) else {
        return;
    };

    for d in due {
        let id = d.id;
        if let Ok(store) = state.store.lock() {
            let _ = store.set_send_state(id, SendState::Transmitting, None, None, None);
        }
        let to: Vec<String> =
            d.to.split([',', ';'])
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
        let attempts = {
            state
                .store
                .lock()
                .ok()
                .and_then(|s| s.outbox(account).ok())
                .and_then(|rows| rows.into_iter().find(|r| r.id == id).map(|r| r.attempts))
                .unwrap_or(0)
        };

        let cc: Vec<String> =
            d.cc.split([',', ';'])
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
        // The whole message, not only its text: a reply that waited in the
        // undo window still threads into its conversation, and still carries
        // what was attached to it.
        let result = attempt(
            &state,
            to,
            cc,
            d.subject,
            d.body,
            // Stored alongside the text, so a message posted hours ago goes
            // out in the form it was written rather than flattened by the wait.
            Some(d.html).filter(|h| !h.trim().is_empty()),
            d.envelope.in_reply_to,
            d.envelope.references,
            d.envelope.attachments,
        )
        .await;

        let (outcome, detail, message_id) = match result {
            Ok(a) => (a.outcome, a.detail, Some(a.message_id)),
            // Could not even assemble the message — a missing attachment, no
            // account. Not a transport verdict, so it is treated as a failure
            // before anything was committed: safe to retry once it is fixed.
            Err(e) => (AttemptOutcome::FailedBeforeCommit, e, None),
        };

        // Evidence is consulted only when the transport could not say.
        let evidence = if outcome == AttemptOutcome::UnknownAfterTransmit {
            match &message_id {
                Some(m) => sent_folder_evidence(&state, &cfg, account, m).await,
                None => ServerEvidence::Indeterminate,
            }
        } else {
            ServerEvidence::Indeterminate
        };
        let next = reconcile(outcome, evidence);

        let Ok(store) = state.store.lock() else {
            continue;
        };
        match next {
            SendState::Sent => {
                log_sync(&format!(
                    "queued send delivered {}",
                    message_id.as_deref().unwrap_or("?")
                ));
                drop_server_draft_using(
                    &store,
                    id,
                    state.server_has_uidplus.load(Ordering::Relaxed),
                );
                let _ = store.delete_draft(id);
            }
            SendState::RetryQueued => {
                let n = attempts + 1;
                let wait = retry_delay_ms(n);
                log_sync(&format!(
                    "queued send failed, retrying in {}s: {detail}",
                    wait / 1000
                ));
                let _ = store.set_send_state(
                    id,
                    SendState::RetryQueued,
                    Some(&detail),
                    Some(now_ms() + wait),
                    message_id.as_deref(),
                );
            }
            SendState::FailedPermanent => {
                log_sync(&format!("queued send rejected: {detail}"));
                let _ = store.set_send_state(
                    id,
                    SendState::FailedPermanent,
                    Some(&detail),
                    None,
                    message_id.as_deref(),
                );
            }
            SendState::NeedsAttention => {
                // The one outcome no amount of engineering resolves. Said in
                // the row, and raised as a notification, because silence is
                // the one response that loses mail.
                log_sync(&format!("queued send needs attention: {detail}"));
                let _ = store.set_send_state(
                    id,
                    SendState::NeedsAttention,
                    Some(&detail),
                    None,
                    message_id.as_deref(),
                );
            }
            SendState::UndoWindow | SendState::Transmitting => {}
        }
    }
}

/// Step 1 → 2 of onboarding: what an address tells us about its servers.
#[tauri::command]
async fn discover_account(
    address: String,
) -> Result<Option<petrel_autoconfig::Discovered>, String> {
    petrel_autoconfig::discover(&address)
        .await
        .map_err(|e| e.to_string())
}

/// The manual form's pre-fill when nothing answered: the conventional hosts.
#[tauri::command]
fn guess_servers(
    address: String,
) -> Option<(petrel_autoconfig::Server, petrel_autoconfig::Server)> {
    petrel_autoconfig::guess(&address)
}

#[derive(serde::Deserialize)]
struct AccountSetup {
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
async fn test_account(setup: AccountSetup, which: Option<String>) -> Result<(), String> {
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
        pass: setup.password.clone(),
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
        pass: setup.password.clone(),
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
fn add_account(setup: AccountSetup, state: State<Arc<AppState>>) -> Result<i64, String> {
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
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        // The row the environment made, if that is what is here, gives way:
        // an account set up in the app is the account.
        if let Ok(Some(first)) = store.first_account() {
            if store.account_servers(first).ok().flatten().is_none()
                && imap_config_from_env().is_none()
            {
                let _ = store.remove_account(first);
            }
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
fn set_active_account(account_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
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
fn remove_account(account_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    if let Ok(k) = keychain_entry(account_id) {
        // A missing entry is fine; the point is that none remains.
        let _ = k.delete_credential();
    }
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.remove_account(account_id).map_err(|e| e.to_string())
}

/// The bytes of one attachment, re-read from the message's raw blob.
///
/// Nothing is stored twice: the raw message holds every attachment, and the
/// part is decoded when asked for — on save, on open, on preview.
fn attachment_bytes(
    state: &AppState,
    message_id: i64,
    part: usize,
) -> Result<(petrel_mime::Attachment, Vec<u8>), String> {
    let hash = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message body not stored")?
    };
    let raw = state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable (failed verification)")?;
    petrel_mime::attachment_bytes(&raw, part)
        .ok_or_else(|| "that attachment is not in the message".into())
}

/// What the message offers for leaving its list, shaped for the UI.
#[derive(serde::Serialize)]
struct UnsubInfo {
    /// True when RFC 8058 one-click is available — leaving without opening
    /// anything, which is the safest of the three.
    one_click: bool,
    url: Option<String>,
    mailto: Option<String>,
}

fn raw_message_of(state: &AppState, message_id: i64) -> Result<Vec<u8>, String> {
    let hash = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message body not stored")?
    };
    state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable (failed verification)".into())
}

/// Reads the List-Unsubscribe offer for one message, if it makes one.
#[tauri::command]
fn unsubscribe_info(
    message_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<UnsubInfo>, String> {
    let raw = raw_message_of(&state, message_id)?;
    Ok(petrel_mime::unsubscribe_info(&raw).map(|u| UnsubInfo {
        one_click: u.one_click.is_some(),
        url: u.url,
        mailto: u.mailto,
    }))
}

/// Sends the RFC 8058 one-click POST for this message.
///
/// The URL is re-derived from the message's own bytes rather than accepted
/// from the caller: the message is the authority on where its list lives,
/// and a bridge that POSTs to whatever URL it is handed is a resource any
/// page in the webview would love to have.
#[tauri::command]
async fn unsubscribe_one_click(
    message_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let raw = raw_message_of(&state, message_id)?;
    let url = petrel_mime::unsubscribe_info(&raw)
        .and_then(|u| u.one_click)
        .ok_or("this message does not offer one-click unsubscribe")?;
    tauri::async_runtime::spawn_blocking(move || post_one_click(&url))
        .await
        .map_err(|e| e.to_string())?
}

/// The POST itself: the fixed form body RFC 8058 specifies, over https only.
fn post_one_click(url: &str) -> Result<(), String> {
    if !url.to_ascii_lowercase().starts_with("https://") && !cfg!(test) {
        return Err("one-click unsubscribe must be https".into());
    }
    ureq::post(url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send("List-Unsubscribe=One-Click")
        .map(|_| ())
        .map_err(|e| format!("the sender's unsubscribe endpoint refused: {e}"))
}

/// File types that run when opened. Opening one is a real decision — the
/// spec asks for a warning, and the UI asks before calling `open_attachment`
/// on any of these — so the list lives here, next to the thing it guards.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "pif", "ps1", "vbs", "vbe", "js", "jse", "wsf",
    "wsh", "hta", "jar", "app", "dmg", "pkg", "command", "sh", "zsh", "bash", "csh", "py", "rb",
    "pl", "php", "apk", "deb", "rpm", "appimage", "lnk", "url", "reg", "scpt", "action",
    "workflow", "terminal",
];

/// Whether a file name ends in something the OS would execute.
#[tauri::command]
fn attachment_is_executable(filename: String) -> bool {
    let ext = std::path::Path::new(&filename)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext {
        Some(e) => EXECUTABLE_EXTENSIONS.contains(&e.as_str()),
        None => false,
    }
}

/// Writes an attachment to a path the user chose. The dialog is the UI's;
/// this only gets the path it produced.
#[tauri::command]
fn save_attachment(
    message_id: i64,
    part: usize,
    path: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let (_, bytes) = attachment_bytes(&state, message_id, part)?;
    std::fs::write(&path, bytes).map_err(|e| format!("could not write {path}: {e}"))
}

/// Opens an attachment in whatever the OS uses for its type.
///
/// Written to a per-launch temporary directory first, under its own name
/// so the application that opens it sees the right extension. The file is
/// quarantined the way a download is — macOS then shows its own "downloaded
/// from the internet" prompt for anything it considers risky, on top of the
/// warning the UI has already shown for executables.
#[tauri::command]
fn open_attachment(
    message_id: i64,
    part: usize,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let (meta, bytes) = attachment_bytes(&state, message_id, part)?;
    let name = meta
        .filename
        .as_deref()
        .and_then(|f| std::path::Path::new(f).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "attachment".to_string());
    let dir = std::env::temp_dir().join(format!("petrel-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A subdirectory per message and part, so two attachments that share a
    // name do not overwrite each other while both are open.
    let dir = dir.join(format!("{message_id}-{part}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        // The quarantine attribute is what makes Gatekeeper treat this as a
        // download. Best effort: a file the OS cannot mark is still opened,
        // since the UI's own warning has already been shown.
        let _ = std::process::Command::new("xattr")
            .arg("-w")
            .arg("com.apple.quarantine")
            .arg("0083;00000000;Petrel;")
            .arg(&path)
            .status();
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A one-use URL for previewing an attachment in the reading pane, over the
/// same sandboxed protocol that serves message bodies.
#[tauri::command]
fn attachment_url(message_id: i64, part: usize, state: State<Arc<AppState>>) -> String {
    format!(
        "petrel-msg://localhost/attachment/{}/{part}",
        state.tokens.issue(message_id)
    )
}

/// The outbox, row by row, with each message's state.
#[tauri::command]
fn list_outbox(
    state: State<Arc<AppState>>,
) -> Result<Vec<petrel_engine::store::OutboxRow>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.outbox(account).map_err(|e| e.to_string())
}

/// "Send now", "Try now", "Send anyway". The person has looked and decided,
/// which is the only thing that may move a message out of `NeedsAttention` —
/// so this is also the one place that does.
#[tauri::command]
fn outbox_send_now(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store.resend_now(id, now_ms()).map_err(|e| e.to_string())?;
    }
    // Wake the worker so "now" means now, not the next poll.
    state.drain_signal.notify_one();
    Ok(())
}

/// "Edit": back to Drafts with the text intact, out of the queue.
#[tauri::command]
fn outbox_edit(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.unschedule_send(id).map_err(|e| e.to_string())
}

/// "Check again" for a message whose outcome is unknown: look in Sent once
/// more and resolve it if the evidence is now there. Never sends.
#[tauri::command]
async fn outbox_check(id: i64, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    use petrel_engine::outbox::{AttemptOutcome, SendState, reconcile};
    let (account, message_id) = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let account = store
            .active_account()
            .map_err(|e| e.to_string())?
            .ok_or("no account")?;
        let row = store
            .outbox(account)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("that message is no longer in the outbox")?;
        let mid: Option<String> = store
            .conn_query_send_message_id(id)
            .map_err(|e| e.to_string())?;
        (account, mid.filter(|_| row.state == "NeedsAttention"))
    };
    let Some(mid) = message_id else {
        return Ok("Indeterminate".into());
    };
    let cfg = imap_config(&state, account).ok_or("no account is configured")?;
    let evidence = sent_folder_evidence(&state, &cfg, account, &mid).await;
    let next = reconcile(AttemptOutcome::UnknownAfterTransmit, evidence);
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    match next {
        SendState::Sent => {
            drop_server_draft_using(&store, id, state.server_has_uidplus.load(Ordering::Relaxed));
            let _ = store.delete_draft(id);
        }
        SendState::RetryQueued => {
            let _ = store.resend_now(id, now_ms());
            state.drain_signal.notify_one();
        }
        _ => {}
    }
    Ok(format!("{next:?}"))
}

/// Saves the composer's contents so they survive closing it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn save_draft(
    draft_id: Option<i64>,
    to: String,
    cc: Option<String>,
    subject: String,
    body: String,
    html: String,
    in_reply_to: Option<String>,
    references: Option<Vec<String>>,
    attachments: Option<Vec<String>>,
    state: State<Arc<AppState>>,
) -> Result<i64, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    let envelope = petrel_engine::store::DraftEnvelope {
        in_reply_to,
        references: references.unwrap_or_default(),
        attachments: attachments.unwrap_or_default(),
    };
    let id = store
        .save_draft_full(
            account,
            draft_id,
            &to,
            cc.as_deref().unwrap_or(""),
            &subject,
            &body,
            &html,
            &envelope,
        )
        .map_err(|e| e.to_string())?;
    drop(store);
    // The server copy follows on the 30-second clock; closing the composer
    // pushes at once through `push_draft` instead of waiting it out.
    schedule_draft_push(Arc::clone(state.inner()), id);
    Ok(id)
}

/// Pushes the draft's current text to the server now — the composer closing
/// is the one moment the debounce must not be allowed to lose.
#[tauri::command]
async fn push_draft(id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Ok(mut dirty) = state.draft_dirty.lock() {
        dirty.remove(&id);
    }
    push_draft_to_server(state.inner(), id).await
}

#[tauri::command]
fn load_draft(id: i64, state: State<Arc<AppState>>) -> Result<DraftRecord, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let record = store.load_draft(id).map_err(|e| e.to_string())?;
    if !record.body.is_empty() || !record.html.is_empty() {
        return Ok(record);
    }
    // A draft written in another client: it arrived through folder sync as a
    // message, so its words live in the raw blob rather than in the draft
    // columns. Reconstruct the composer's view from the message itself.
    // (Attachments stay with the server copy for now — the words are what a
    // draft is; reattaching is a save away.)
    let Some(hash) = store.blob_hash_for(id).ok().flatten() else {
        return Ok(record);
    };
    let Ok(raw) = state.blobs.read(&hash) else {
        return Ok(record);
    };
    let Some(parsed) = petrel_mime::parse_message(&raw) else {
        return Ok(record);
    };
    let join = |list: &[(Option<String>, String)]| {
        list.iter()
            .map(|(_, addr)| addr.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Ok(DraftRecord {
        id,
        to: join(&parsed.to),
        cc: join(&parsed.cc),
        subject: parsed.subject.clone().unwrap_or_default(),
        body: parsed.body_text.clone(),
        html: parsed
            .body_html
            .clone()
            .unwrap_or_else(|| petrel_mime::plain_text_to_html(&parsed.body_text)),
        envelope: petrel_engine::store::DraftEnvelope {
            in_reply_to: parsed.references.last().cloned().map(|r| format!("<{r}>")),
            references: parsed.references.iter().map(|r| format!("<{r}>")).collect(),
            attachments: Vec::new(),
        },
    })
}

#[tauri::command]
fn delete_draft(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    // The server's copy goes with it. Read before the local row disappears.
    spawn_drop_server_draft(state.inner(), id);
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.delete_draft(id).map_err(|e| e.to_string())
}

/// Name and size for files the user picked, so the composer can refuse an
/// oversized one before the message is written.
///
/// Statted here rather than in the window: the file picker hands back paths,
/// and asking the OS for a size is something the backend can already do
/// without a second plugin and a second capability to review.

/// Writes a dropped file to disk and reports where it landed.
///
/// A file picked from the dialog arrives as a path, because the dialog is the
/// system's and hands one over. A file dragged in from the desktop does not:
/// the webview gives the page bytes and deliberately withholds the path, so
/// there is nothing for the sender to open later. Staging it is what turns the
/// one into the other, and means everything downstream — the size rule, the
/// list in the composer, the send itself — keeps working on paths and does not
/// learn that drops exist.
///
/// The name is reduced to a file name and nothing else. It arrives from a drag
/// the application did not compose, so `../../.ssh/id_rsa` has to be a file
/// called `id_rsa` in the staging directory and not a path out of it.
#[tauri::command]
fn stage_attachment(name: String, bytes: Vec<u8>) -> Result<AttachmentInfo, String> {
    let stem = std::path::Path::new(&name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "attachment".to_string());

    let dir = data_dir().join("staged");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    // Prefixed rather than overwritten: dropping two files of the same name
    // from different folders is ordinary, and the second must not replace the
    // first after the first is already listed in the composer.
    let unique = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        stem
    );
    let path = dir.join(unique);
    let size = bytes.len() as u64;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;

    Ok(AttachmentInfo {
        name: stem,
        size,
        path: path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
fn attachment_info(paths: Vec<String>) -> Vec<AttachmentInfo> {
    paths
        .into_iter()
        .map(|path| {
            let p = std::path::Path::new(&path);
            AttachmentInfo {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone()),
                // Unreadable reports zero rather than failing the whole pick;
                // the send will report it properly if it is still a problem.
                size: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                path,
            }
        })
        .collect()
}

#[derive(serde::Serialize)]
struct AttachmentInfo {
    path: String,
    name: String,
    size: u64,
}

/// A content type from the file extension.
///
/// Deliberately a short list plus a catch-all. Guessing wrong is harmless —
/// application/octet-stream always works and every client offers to save it —
/// whereas a large mapping table is a lot of lines that can only be subtly
/// wrong. The types here are the ones people actually attach.
fn guess_content_type(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Who mail is sent as, and what goes underneath it.
#[tauri::command]
fn get_identity(state: State<Arc<AppState>>) -> Result<Identity, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    store.identity(account).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_identity(
    display_name: String,
    signature: String,
    signature_on_reply: bool,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    store
        .set_identity(account, &display_name, &signature, signature_on_reply)
        .map_err(|e| e.to_string())
}

/// What the Storage pane shows.
#[tauri::command]
fn storage_report(state: State<Arc<AppState>>) -> Result<StorageReport, String> {
    let blob_bytes = state.blobs.total_bytes().unwrap_or(0);
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .storage_report(
            &std::path::Path::new(&state.data_dir).join("petrel.db"),
            blob_bytes,
        )
        .map_err(|e| e.to_string())
}

/// The active account's filter rules, in run order.
#[tauri::command]
fn list_rules(state: State<Arc<AppState>>) -> Result<Vec<petrel_engine::rules::Rule>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.rules_for_account(account).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_rule(
    rule_id: Option<i64>,
    name: String,
    enabled: bool,
    conditions: Vec<petrel_engine::rules::Condition>,
    actions: petrel_engine::rules::Actions,
    state: State<Arc<AppState>>,
) -> Result<i64, String> {
    let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    store
        .save_rule(account, rule_id, &name, enabled, &conditions, &actions)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_rule(rule_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.delete_rule(rule_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn move_rule(rule_id: i64, up: bool, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.move_rule(rule_id, up).map_err(|e| e.to_string())
}

/// Opens a message in its own window as a printable page.
///
/// A window rather than printing the app: the app window is chrome around a
/// sandboxed frame, and printing it prints the chrome. The print window
/// loads the message's printable document over the same protocol, so the
/// same sanitizer, the same CSP and the same remote-content policy govern
/// what lands on paper — and the page opens straight into the print dialog.
#[tauri::command]
fn print_message(
    message_id: i64,
    app: tauri::AppHandle,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let token = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message has no stored body")?;
        state.tokens.issue(message_id)
    };
    let label = format!("print-{message_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }
    let url: tauri::Url = format!("petrel-msg://localhost/print/{token}")
        .parse()
        .map_err(|e| format!("{e}"))?;
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title("Print")
        .inner_size(700.0, 880.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// What an import did, honestly itemised.
#[derive(serde::Serialize)]
struct ImportReport {
    imported: usize,
    /// Already here — same Message-ID. Importing twice is a no-op, not a copy.
    duplicates: usize,
    failed: usize,
}

/// Imports mbox files and .eml messages into a local "Imported" folder.
///
/// Local, marked so: the server has never heard of this folder, so the sync
/// survey must not prune it and the sync loop must not ask about it. The
/// messages carry no UID for the same reason — NULL is already how "not
/// addressable on a server" is spelled here. Dedupe is the ordinary one, by
/// Message-ID, which is what makes a re-import of the same archive report
/// duplicates instead of doubling the mailbox.
#[tauri::command]
fn import_mail(paths: Vec<String>, state: State<Arc<AppState>>) -> Result<ImportReport, String> {
    let mut report = ImportReport {
        imported: 0,
        duplicates: 0,
        failed: 0,
    };
    let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    let folder = store
        .ensure_named_folder(account, "Imported")
        .map_err(|e| e.to_string())?;
    store.mark_folder_local(folder).map_err(|e| e.to_string())?;

    for path in &paths {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                log_sync(&format!("import: could not read {path}: {e}"));
                report.failed += 1;
                continue;
            }
        };
        let messages: Vec<Vec<u8>> = if path.to_ascii_lowercase().ends_with(".eml") {
            vec![bytes]
        } else {
            petrel_engine::mbox::split(&bytes)
        };
        for raw in &messages {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                store.ingest_raw(&state.blobs, account, Some(folder), None, raw)
            }));
            match outcome {
                Ok(Ok(ingested)) if ingested.was_new => {
                    report.imported += 1;
                    state.seeded.fetch_add(1, Ordering::Relaxed);
                }
                Ok(Ok(_)) => report.duplicates += 1,
                Ok(Err(e)) => {
                    log_sync(&format!("import: one message failed: {e}"));
                    report.failed += 1;
                }
                Err(_) => {
                    log_sync("import: one message PANICKED the parser — skipped");
                    report.failed += 1;
                }
            }
        }
    }
    log_sync(&format!(
        "import: {} new, {} duplicate(s), {} failed",
        report.imported, report.duplicates, report.failed
    ));
    Ok(report)
}

/// Writes a view's mail to an mbox file the user chose.
///
/// The path comes from the OS save panel rather than a location Petrel picks:
/// an export is something you take somewhere, and guessing where would make the
/// durability promise depend on knowing where Petrel hides things.
#[tauri::command]
fn export_mbox(
    view: Option<String>,
    path: String,
    state: State<Arc<AppState>>,
) -> Result<String, String> {
    let view = ListView::parse(view.as_deref().unwrap_or("inbox"));
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let (written, skipped) = store
        .export_mbox(&state.blobs, &view, std::path::Path::new(&path))
        .map_err(|e| e.to_string())?;
    log_sync(&format!(
        "exported {written} message(s) to mbox, {skipped} skipped"
    ));
    Ok(format!("{written}/{skipped}"))
}

/// Folders for the move picker (V).
#[tauri::command]
fn list_folders(state: State<Arc<AppState>>) -> Result<Vec<FolderSummary>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.folders(account).map_err(|e| e.to_string())
}

/// Creates a folder the user named, or returns the one already there. The
/// picker offers this on the end of the same keystroke as choosing one.
#[tauri::command]
fn create_folder(path: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let (account, id, cfg) = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let account = store
            .active_account()
            .map_err(|e| e.to_string())?
            .ok_or("no account")?;
        let id = store
            .ensure_named_folder(account, &path)
            .map_err(|e| e.to_string())?;
        (account, id, imap_config_for(&store, account))
    };
    let _ = account;
    // The server's copy follows, off this thread — the picker is waiting on
    // the id, and a move drained later re-creates on demand anyway, so the
    // worst a failure here costs is that retry.
    if let Some(cfg) = cfg {
        tauri::async_runtime::spawn(async move {
            if let Err(e) = petrel_providers::imap::create_folder(&cfg, &path).await {
                log_sync(&format!("server create {path} failed: {e}"));
            }
        });
    }
    Ok(id)
}

/// Renames a folder — on the server first, then locally, so the two cannot
/// disagree with the server holding the older name.
#[tauri::command]
async fn rename_folder(
    folder_id: i64,
    new_path: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let (cfg, old_path) = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let account = store
            .active_account()
            .map_err(|e| e.to_string())?
            .ok_or("no account")?;
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        (imap_config_for(&store, account), path)
    };
    if let Some(cfg) = cfg {
        petrel_providers::imap::rename_folder(&cfg, &old_path, &new_path)
            .await
            .map_err(|e| e.to_string())?;
    }
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .rename_folder(folder_id, &new_path)
        .map_err(|e| e.to_string())
}

/// Deletes a folder — on the server first. The server also deletes whatever
/// mail the folder still holds, which is why the UI confirms in those words;
/// the store keeps its message rows and blobs regardless, so nothing already
/// synced is destroyed.
#[tauri::command]
async fn delete_folder(folder_id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let (cfg, path) = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let account = store
            .active_account()
            .map_err(|e| e.to_string())?
            .ok_or("no account")?;
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        (imap_config_for(&store, account), path)
    };
    if let Some(cfg) = cfg {
        petrel_providers::imap::delete_folder(&cfg, &path)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.remove_folder(folder_id).map_err(|e| e.to_string())
}

/// Creates a tag, or returns the one already there — same shape as folders.
#[tauri::command]
fn create_tag(name: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    store
        .ensure_tag(account, &name, None)
        .map_err(|e| e.to_string())
}

/// Corrects a tag's name. The colour and every tagged message come with it.
#[tauri::command]
fn rename_tag(tag_id: i64, name: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.rename_tag(tag_id, &name).map_err(|e| e.to_string())
}

/// Sets a tag's colour. Local by design: no provider has a field for it.
#[tauri::command]
fn set_tag_colour(tag_id: i64, colour: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .set_tag_colour(tag_id, &colour)
        .map_err(|e| e.to_string())
}

/// Removes a tag from the account and from every message carrying it.
#[tauri::command]
fn delete_tag(tag_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.delete_tag(tag_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn undo_triage(action_id: i64, state: State<Arc<AppState>>) -> Result<bool, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let undone = store.undo_action(action_id).map_err(|e| e.to_string())?;
    // An undo can leave other queued work behind it, and the row it cancelled
    // is gone from the queue — either way the server's picture just changed.
    state.drain_signal.notify_one();
    Ok(undone)
}

#[tauri::command]
fn list_accounts(state: State<Arc<AppState>>) -> Result<Vec<AccountSummary>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.accounts().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_account_color(
    account_id: i64,
    color: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .set_account_color(account_id, &color)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_account_archive(
    account_id: i64,
    enabled: bool,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .set_local_archive(account_id, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(state: State<Arc<AppState>>) -> Result<HashMap<String, String>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.settings().map_err(|e| e.to_string())
}

/// An empty value clears the preference, restoring the default rather than
/// pinning the current one.
#[tauri::command]
fn set_setting(key: String, value: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    if value.is_empty() {
        store.clear_setting(&key).map_err(|e| e.to_string())
    } else {
        store.set_setting(&key, &value).map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn search_messages(
    query: String,
    newest: bool,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadListing>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .search_threads_sorted(&query, 200, newest)
        .map_err(|e| e.to_string())
}

/// Issues a one-message URL for the reading pane. The UI never receives the
/// body over IPC — bulk bytes go over the custom protocol, and the frame that
/// renders them has no IPC access at all.
/// The original of a message, ready to be quoted in a reply.
#[derive(serde::Serialize)]
struct Quoted {
    html: String,
    text: String,
    from: String,
    date_ms: i64,
    /// The message's own recipients and subject, for a forward's header block.
    /// Taken from the message rather than from the conversation: a thread's
    /// subject drifts, and forwarding one message out of the middle of it
    /// should say what *that* message said.
    to: String,
    subject: String,
}

/// Reads a message back for quoting.
///
/// Sanitized before it leaves, and with remote content stripped — not because
/// the composer would render it, but because whatever is quoted is about to be
/// *sent*. Quoting a tracked message with its pixel intact would forward that
/// pixel to everyone on the reply and fire it again for each of them, turning
/// the person replying into the tracker's delivery mechanism.
#[tauri::command]
fn quote_message(message_id: i64, state: State<Arc<AppState>>) -> Result<Quoted, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let hash = store
        .blob_hash_for(message_id)
        .map_err(|e| e.to_string())?
        .ok_or("message has no stored body")?;
    let raw = state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable")?;
    let parsed = petrel_mime::parse_message(&raw).ok_or("message could not be parsed")?;

    let (from, date_ms) = store
        .message_header(message_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();

    let html = match parsed.body_html.as_deref() {
        Some(h) => petrel_mime::sanitize_html(h, false).html,
        // No HTML half: the text becomes the quote, escaped into paragraphs so
        // it arrives as prose rather than as one run-on line.
        None => petrel_mime::plain_text_to_html(&parsed.body_text),
    };

    let to = parsed
        .to
        .iter()
        .map(|(name, addr)| match name {
            Some(n) if !n.trim().is_empty() => format!("{n} <{addr}>"),
            _ => addr.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");

    Ok(Quoted {
        html,
        text: parsed.body_text,
        from,
        date_ms,
        to,
        subject: parsed.subject.clone().unwrap_or_default(),
    })
}

#[tauri::command]
fn message_url(message_id: i64, state: State<Arc<AppState>>) -> Result<String, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    match store.blob_hash_for(message_id).map_err(|e| e.to_string())? {
        Some(_) => Ok(format!(
            "petrel-msg://localhost/message/{}",
            state.tokens.issue(message_id)
        )),
        None => Err("message has no stored body".into()),
    }
}

fn spawn_demo_seeding(state: Arc<AppState>, account: i64) {
    std::thread::spawn(move || {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // DemoMailbox, not MailboxGen: the latter generates word-salad on
        // purpose, because search-recall benchmarks need rare tokens and a flat
        // distribution. That is the wrong corpus for looking at the UI, where
        // noise hides exactly the problems you are trying to see.
        let mut generator = DemoMailbox::new(7, DEMO_MESSAGES, now);
        loop {
            let batch: Vec<NewMessage> = generator
                .by_ref()
                .take(500)
                .map(|g| NewMessage {
                    account_id: account,
                    date_ms: g.date_ms,
                    from_addr: g.from_addr,
                    from_display: g.from_display,
                    to_addr: g.to_addr,
                    subject: g.subject,
                    body_text: g.body,
                })
                .collect();
            if batch.is_empty() {
                break;
            }
            let n = batch.len();
            match state.store.lock() {
                Ok(mut store) => {
                    if store.insert_messages(&batch).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
            state.seeded.fetch_add(n, Ordering::Relaxed);
        }
        state.seeding.store(false, Ordering::Relaxed);
    });
}

/// Demo decoration for a store that holds synthetic mail: tags, read state, a
/// few stars and attachments, so the list shows what the design describes
/// instead of 10,000 identically-unread rows.
///
/// Runs once, guarded by a meta key, and **only when no real account is
/// configured** — this writes flags, and flags on real mail belong to the
/// server, not to a demo routine.
fn reseed_demo_if_stale(state: &Arc<AppState>, account: i64) -> bool {
    const WANT: &str = "3";
    let synthetic = {
        let Ok(store) = state.store.lock() else {
            return false;
        };
        if store.meta("demo_seed_version").ok().flatten().as_deref() == Some(WANT) {
            return false;
        }
        // Only ever touches a store that is *entirely* synthetic. One real
        // message and this does nothing — deleting somebody's mail to improve a
        // demo would be an unforgivable trade.
        store.all_messages_synthetic().unwrap_or(false)
    };
    if !synthetic {
        return false;
    }
    match state.store.lock() {
        Ok(store) => {
            let removed = store.delete_all_messages().unwrap_or(0);
            let _ = store.set_meta("demo_seed_version", "3");
            let _ = store.set_meta("demo_decorated", "");
            eprintln!("[demo] cleared {removed} synthetic messages for a fresh seed");
        }
        Err(_) => return false,
    }
    state.seeded.store(0, Ordering::Relaxed);
    state.seeding.store(true, Ordering::Relaxed);
    spawn_demo_seeding(state.clone(), account);
    true
}

fn decorate_demo_store(state: &Arc<AppState>, account: i64) {
    let Ok(store) = state.store.lock() else {
        return;
    };
    if store
        .meta("demo_decorated")
        .ok()
        .flatten()
        .is_some_and(|v| !v.is_empty())
    {
        return;
    }
    let tags: Vec<(i64, u32)> = [
        ("urgent", "#B0524A", 7u32),
        ("receipts", "#5E7C4A", 11),
        ("read later", "#9A6B1F", 17),
    ]
    .iter()
    .filter_map(|(name, colour, every)| {
        store
            .ensure_tag(account, name, Some(colour))
            .ok()
            .map(|id| (id, *every))
    })
    .collect();

    let ids: Vec<i64> = match store.recent_ids(4000) {
        Ok(v) => v,
        Err(_) => return,
    };
    for (i, id) in ids.iter().enumerate() {
        // Most mail has been read; a scattering has not.
        if i % 6 != 0 {
            let _ = store.set_flags(*id, petrel_engine::store::flags::SEEN, 0);
        }
        if i % 23 == 0 {
            let _ = store.set_flags(*id, petrel_engine::store::flags::FLAGGED, 0);
        }
        if i % 9 == 0 {
            let _ = store.set_has_attachments(*id, true);
        }
        for (tag_id, every) in &tags {
            if (i as u32).is_multiple_of(*every) {
                let _ = store.tag_message(*id, *tag_id);
            }
        }
    }
    // A mailbox without folders is not a mailbox: triage has nowhere to move
    // mail to, and the folder mapping pane has nothing to report.
    for (role, path) in [
        ("inbox", "INBOX"),
        ("archive", "Archive"),
        ("sent", "Sent"),
        ("drafts", "Drafts"),
        ("spam", "Junk"),
        ("trash", "Trash"),
    ] {
        let _ = store.ensure_folder(account, role, path);
    }
    if let Ok(Some(inbox)) = store.folder_for_role(account, "inbox") {
        for id in &ids {
            let _ = store.place_message(*id, inbox);
        }
    }

    let _ = store.set_meta("demo_decorated", "1");
    eprintln!(
        "[demo] decorated {} messages with tags and flags",
        ids.len()
    );
}

/// Webview-side diagnostics: init scripts run before page scripts and are
/// exempt from page CSP, so this reports what the webview actually did (loaded
/// URL, script execution, errors, CSP violations) even when the page itself is
/// dead. Events land on stderr via `frontend_log`.
const DIAG: &str = r#"
(function () {
  var buf = [];
  function flush() {
    if (!window.__TAURI_INTERNALS__ || !window.__TAURI_INTERNALS__.invoke) { setTimeout(flush, 50); return; }
    while (buf.length) {
      var e = buf.shift();
      try { window.__TAURI_INTERNALS__.invoke('frontend_log', { entry: e }); } catch (err) {}
    }
  }
  function send(obj) { try { buf.push(JSON.stringify(obj)); } catch (e) { buf.push('"unserializable"'); } flush(); }

  // What remains of the diagnostics is the part that still earns its place:
  // uncaught errors. The input and focus probes below this were scaffolding for
  // a window that would not respond, which was traced to the launch context
  // months of debugging ago; left in, they wrote a line every three seconds
  // forever and buried the one line that mattered.
  try { document.title = 'D:' + String(location.href).slice(0, 48); } catch (e) {}
  send({ kind: 'boot', href: String(location.href), readyState: document.readyState });
  window.addEventListener('error', function (e) {
    if (e && e.target && e.target !== window && (e.target.src || e.target.href)) {
      send({ kind: 'resource-error', url: String(e.target.src || e.target.href) });
      return;
    }
    send({ kind: 'js-error', msg: String(e.message), src: String(e.filename) + ':' + e.lineno });
  }, true);
  window.addEventListener('unhandledrejection', function (e) { send({ kind: 'rejection', msg: String(e.reason) }); });
  document.addEventListener('securitypolicyviolation', function (e) {
    send({ kind: 'csp-violation', directive: String(e.violatedDirective), blocked: String(e.blockedURI) });
  });
  window.addEventListener('DOMContentLoaded', function () {
    send({ kind: 'dom', scripts: document.scripts.length, root: !!document.getElementById('root') });
    setTimeout(function () {
      var r = document.getElementById('root');
      send({ kind: 'settled', rootChildren: r ? r.childElementCount : -1,
             bodyText: ((document.body && document.body.innerText) || '').slice(0, 80) });
    }, 2000);
  });
})();
"#;

/// Opt-in UI smoke test (`PETREL_SELFTEST=1`): drives the search box the way a
/// user would — real input events into React — and reports what came back.
/// Verifies UI → IPC → engine → FTS → UI end to end without needing OS
/// accessibility permissions. Precursor to the M5 E2E suite.
const SELFTEST: &str = r#"
(function () {
  function log(o) { try { window.__TAURI_INTERNALS__.invoke('frontend_log', { entry: JSON.stringify(o) }); } catch (e) {} }
  function type(el, text) {
    var setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(el, text);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  }
  function rows() { return document.querySelectorAll('.row').length; }
  function timing() { var m = document.querySelectorAll('.meta span'); return m.length > 1 ? m[1].textContent : ''; }
  function firstRow() { var r = document.querySelector('.row'); return r ? r.innerText.replace(/\s+/g, ' ').slice(0, 90) : ''; }
  var queries = (window.__PETREL_SELFTEST_QUERIES__ || ['meeting', 'zephyrite5000', '東京計', 'quarterly report']);
  var i = 0;
  function step() {
    var input = document.querySelector('.search');
    if (!input) { setTimeout(step, 300); return; }
    if (i >= queries.length) {
      // Open the first result so the reading pane renders under observation.
      if (window.__PETREL_SELFTEST_OPEN__) {
        var row = document.querySelector('.row');
        if (row) { row.click(); }
        setTimeout(function () {
          var f = document.querySelector('.reader iframe');
          log({ kind: 'selftest-open', opened: !!f, src: f ? f.getAttribute('src') : null,
                sandbox: f ? f.getAttribute('sandbox') : null });
        }, 1500);
      }
      log({ kind: 'selftest-done' });
      return;
    }
    var q = queries[i++];
    type(input, q);
    setTimeout(function () {
      log({ kind: 'selftest', query: q, results: rows(), timing: timing(), first: firstRow() });
      step();
    }, 900);
  }
  setTimeout(step, 4000);
})();
"#;

#[tauri::command]
fn frontend_log(entry: String) {
    eprintln!("[frontend] {entry}");
    // Also to a file: when the app is launched through LaunchServices (an .app
    // bundle, which is the only way macOS gives it real focus) stderr goes
    // nowhere readable, and diagnostics that vanish are not diagnostics.
    let path = data_dir().join("frontend.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{entry}");
    }
}

/// Where mail lives on disk. Shown in the UI so "your mail is yours" is a
/// path the user can open, not a slogan.
fn data_dir() -> std::path::PathBuf {
    std::env::var("PETREL_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Petrel")
        })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Chosen once, here, for the whole process. rustls picks a crypto provider
    // on its own only while exactly one is compiled in; the moment a second
    // dependency brought another, every TLS handshake that ran before the sync
    // had warmed one up panicked — which is to say, the onboarding connection
    // test on a first run. An application is supposed to say which it wants.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("create data directory");
    eprintln!("[store] data directory: {}", dir.display());

    let store = Store::open(&dir.join("petrel.db")).expect("open store");
    // One account row for now; the account model arrives with setup UI.
    let account = match store.first_account().expect("read accounts") {
        Some(id) => id,
        None => store.ensure_test_account().expect("create account row"),
    };
    // Name it after the account actually configured, before any sync runs — the
    // address is known from the environment, so there is no reason for the
    // switcher to say test@example.com while real mail is arriving.
    if let Some(cfg) = imap_config_from_env() {
        if let Err(e) = store.set_account_email(account, &cfg.user) {
            eprintln!("[store] could not name the account: {e}");
        }
    }
    // Accounts made before colours were assigned at creation wear none, and
    // show as grey dots nobody can tell apart. Each gets the next free one.
    for id in store.account_ids().unwrap_or_default() {
        let _ = store.ensure_account_colour(id);
    }
    let blobs = BlobStore::open(&dir.join("blobs")).expect("open blob store");

    // Startup housekeeping: clear temp files left by an interrupted write, then
    // destroy anything whose grace period expired while the app was closed.
    let _ = blobs.sweep_tmp();
    let state = Arc::new(AppState {
        store: Mutex::new(store),
        blobs,
        seeding: AtomicBool::new(true),
        seeded: AtomicUsize::new(0),
        source: Mutex::new("starting…".into()),
        sync_error: Mutex::new(None),
        drain_signal: Arc::new(tokio::sync::Notify::new()),
        draining: AtomicBool::new(false),
        draft_dirty: Mutex::new(std::collections::HashSet::new()),
        server_has_move: AtomicBool::new(false),
        server_has_uidplus: AtomicBool::new(false),
        server_is_gmail: AtomicBool::new(false),
        server_total: std::sync::atomic::AtomicUsize::new(0),
        shown_once: Mutex::new(std::collections::HashSet::new()),
        tokens: Arc::new(ViewTokens::new()),
        account_id: account,
        data_dir: dir.display().to_string(),
    });

    {
        let now = now_ms();
        if let Ok(mut store) = state.store.lock() {
            match store.gc(
                &state.blobs,
                now,
                petrel_engine::retention::DEFAULT_GRACE_DAYS,
            ) {
                Ok(r) if r.messages_purged > 0 || r.blobs_removed > 0 => eprintln!(
                    "[store] gc purged {} message(s), reclaimed {} blob(s)",
                    r.messages_purged, r.blobs_removed
                ),
                Ok(_) => {}
                Err(e) => eprintln!("[store] gc failed: {e}"),
            }
        }
    }

    // The account set up in the app first; the environment as the developer
    // override when there is none. Before this every launch without the
    // variables was a demo — which is how demo tags ended up decorating a
    // store full of real mail.
    // Every account set up in the app syncs, each on its own tasks. One is
    // *shown* at a time — that is the switcher's job — but mail arriving for
    // the other should be there, read or not, the moment you switch to it.
    // The environment-driven row is the fallback for the developer case only.
    let mut started = 0;
    let configs: Vec<(i64, ImapConfig)> = state
        .store
        .lock()
        .ok()
        .map(|s| {
            s.account_ids()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|id| imap_config_for(&s, id).map(|c| (id, c)))
                .collect()
        })
        .unwrap_or_default();
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
                *state.source.lock().unwrap() =
                    "no account configured · showing stored mail".into();
                if !reseed_demo_if_stale(&state, account) {
                    decorate_demo_store(&state, account);
                }
            } else {
                *state.source.lock().unwrap() = "synthetic demo data".into();
                spawn_demo_seeding(state.clone(), account);
            }
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            status,
            list_messages,
            list_threads,
            thread_by_id,
            open_external,
            stage_attachment,
            list_tags,
            view_counts,
            remote_status,
            show_remote_once,
            trust_sender,
            trusted_senders,
            untrust_sender,
            thread_detail,
            triage,
            undo_triage,
            list_folders,
            create_folder,
            rename_folder,
            delete_folder,
            create_tag,
            rename_tag,
            set_tag_colour,
            delete_tag,
            discover_account,
            guess_servers,
            test_account,
            add_account,
            remove_account,
            set_active_account,
            attachment_is_executable,
            save_attachment,
            open_attachment,
            attachment_url,
            list_outbox,
            outbox_send_now,
            outbox_edit,
            outbox_check,
            send_message,
            storage_report,
            export_mbox,
            import_mail,
            print_message,
            list_rules,
            save_rule,
            delete_rule,
            move_rule,
            get_identity,
            set_identity,
            attachment_info,
            schedule_send,
            popout_compose,
            popout_message,
            complete_addresses,
            quote_message,
            save_draft,
            push_draft,
            unsubscribe_info,
            unsubscribe_one_click,
            load_draft,
            delete_draft,
            list_accounts,
            set_account_color,
            set_account_archive,
            get_settings,
            set_setting,
            search_messages,
            message_url,
            frontend_log
        ])
        .register_uri_scheme_protocol("petrel-msg", move |ctx, request| {
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
            if let Ok(mode) = std::env::var("PETREL_SELFTEST") {
                if mode == "open" {
                    init.push_str(
                        "window.__PETREL_SELFTEST_QUERIES__=['hostile'];\
                         window.__PETREL_SELFTEST_OPEN__=true;",
                    );
                }
                init.push_str(SELFTEST);
            }
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
                .on_navigation(|url| {
                    eprintln!("[nav] {url}");
                    true
                })
                .on_page_load(|_webview, payload| {
                    eprintln!("[pageload] {:?} {}", payload.event(), payload.url());
                })
                .build()?;

            // Say where it actually landed — a window that opens behind another
            // app looks identical to one that failed to open.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
                if let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) {
                    eprintln!(
                        "[window] main at {},{} size {}x{}",
                        pos.x, pos.y, size.width, size.height
                    );
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running petrel");
}

#[cfg(test)]
mod folder_survey_tests {
    use super::without_tag_labels;

    fn rows(v: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        v.iter()
            .map(|(p, r)| (p.to_string(), r.map(|r| r.to_string())))
            .collect()
    }

    #[test]
    fn a_tag_made_here_does_not_come_back_as_a_folder() {
        // The round trip that motivated this: tag "test" → Gmail label
        // "test" → next survey → a folder named "test", the same thing
        // twice pretending to be two.
        let out = without_tag_labels(
            rows(&[("INBOX", Some("inbox")), ("test", None), ("Unwanted", None)]),
            &["test".to_string()],
            true,
        );
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["INBOX", "Unwanted"]);
    }

    #[test]
    fn role_folders_and_other_providers_keep_shared_names() {
        // A Namecheap folder and a tag sharing a name are two real, distinct
        // things — only on Gmail is one object behind both.
        let out = without_tag_labels(
            rows(&[("Receipts", None)]),
            &["Receipts".to_string()],
            false,
        );
        assert_eq!(out.len(), 1);
        // And a role-bearing folder is never a tag, whatever it is called.
        let out = without_tag_labels(
            rows(&[("Starred", Some("starred"))]),
            &["starred".to_string()],
            true,
        );
        assert_eq!(out.len(), 1);
    }
}

#[cfg(test)]
mod unsubscribe_tests {
    use super::post_one_click;
    use std::io::{Read, Write};

    /// The exact bytes RFC 8058 asks for, proven against a listening socket.
    #[test]
    fn the_one_click_post_has_the_shape_the_rfc_specifies() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // Headers and body can arrive in separate reads; keep reading
            // until the body has, or the peer stops.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = sock.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if String::from_utf8_lossy(&buf).contains("One-Click") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf).to_string();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            req
        });
        post_one_click(&format!("http://127.0.0.1:{port}/unsub?u=42")).expect("post");
        let req = served.join().unwrap();
        assert!(req.starts_with("POST /unsub?u=42 HTTP/1.1"), "{req}");
        assert!(
            req.to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded"),
            "{req}"
        );
        assert!(req.ends_with("List-Unsubscribe=One-Click"), "{req}");
    }
}
