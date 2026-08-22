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
    AccountSummary, FolderSummary, ListView, Listing, NewMessage, Store, TagSummary, ThreadListing,
    ThreadMessage,
};
use petrel_providers::imap::{ImapConfig, Security};
use petrel_testkit::DemoMailbox;
use tauri::{Manager, State};

mod message_view;
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
    /// Whether the server supports UID MOVE, learned from the probe.
    server_has_move: AtomicBool,
    tokens: Arc<ViewTokens>,
    account_id: i64,
    data_dir: String,
}

#[derive(serde::Serialize)]
struct Status {
    seeding: bool,
    count: usize,
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
    Status {
        seeding: state.seeding.load(Ordering::Relaxed),
        count: state.seeded.load(Ordering::Relaxed),
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
            drain_actions(Arc::clone(&state), account, cfg.clone(), has_move).await;
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
async fn drain_actions(state: Arc<AppState>, account: i64, cfg: ImapConfig, has_move: bool) {
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
                        petrel_providers::imap::move_uid(&cfg, &folder, uid, &to, has_move).await
                    }
                    // Already where it belongs, or nowhere to send it.
                    _ => Ok(()),
                }
            }
            // Local-only, so they should never have been queued at all — the
            // store marks them 'local' and this drain only reads 'queued'.
            // Handled here so adding a local action later cannot silently fall
            // into the tag branch and be counted as stuck forever.
            ActionKind::Snooze | ActionKind::Unsnooze => continue,
            // Tags are Gmail labels or IMAP keywords depending on the provider,
            // and neither is wired yet. Left queued rather than marked done, so
            // they deliver once that lands instead of being silently dropped.
            ActionKind::Tag | ActionKind::Untag => {
                stuck += 1;
                continue;
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
fn spawn_real_sync(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
    spawn_drain_worker(Arc::clone(&state), account, cfg.clone());
    tauri::async_runtime::spawn(async move {
        *state.source.lock().unwrap() = format!("syncing {}…", cfg.host);

        let mut has_move = false;
        let mut has_idle = false;
        // Folders first. Without them every message ingests with no placement,
        // so the rail's views have nothing to filter on and archiving has
        // nowhere to put anything — which is how a sync can look like it worked
        // while leaving the app unable to file a single message.
        match petrel_providers::imap::probe(&cfg, 0).await {
            Ok(report) => {
                has_move = report.greeting_capabilities.move_;
                has_idle = report.greeting_capabilities.idle;
                state.server_has_move.store(has_move, Ordering::Relaxed);
                log_sync(&format!(
                    "probe ok: {} folder(s), MOVE={has_move}, IDLE={has_idle}",
                    report.folders.len(),
                ));
                let rows: Vec<(String, Option<String>)> = report
                    .folders
                    .iter()
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
                let looks_like_gmail = cfg.host.contains("gmail")
                    || report.folders.iter().any(|f| f.name.starts_with("[Gmail]"));
                if let Ok(store) = state.store.lock() {
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

        let inbox_id = state
            .store
            .lock()
            .ok()
            .and_then(|s| s.folder_for_role(account, "inbox").ok().flatten());

        // Deliver before reading back. Draining first means the server's answer
        // already includes what the user did, so the fetch below confirms local
        // state instead of contradicting it — and anything still queued is
        // protected from being overwritten by the pending checks in the store.
        drain_actions(Arc::clone(&state), account, cfg.clone(), has_move).await;

        // Ingest as each message lands rather than after all of them do. The
        // buffering version showed nothing for the whole fetch, which on a real
        // mailbox is indistinguishable from a hang — and left the list saying
        // the inbox was empty the entire time.
        let limit: u32 = std::env::var("PETREL_SYNC_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        let mut ok = 0usize;
        let mut failed = 0usize;
        let result = {
            let state = Arc::clone(&state);
            petrel_providers::imap::fetch_raw_each(&cfg, "INBOX", limit, |uid, flags, raw| {
                let Ok(mut store) = state.store.lock() else {
                    return;
                };
                match store.ingest_raw(&state.blobs, account, inbox_id, Some(uid), raw) {
                    Ok(ingested) => {
                        // The server's answer about read state wins. Without
                        // this every message arrives unread, so a mailbox with
                        // nothing unread in it shows hundreds.
                        let _ = store.set_message_flags(ingested.message_id, flags);
                        ok += 1;
                        state.seeded.store(ok, Ordering::Relaxed);
                    }
                    Err(_) => failed += 1,
                }
            })
            .await
        };

        match result {
            Ok(seen) => {
                if failed > 0 {
                    log_sync(&format!("{failed} message(s) could not be ingested"));
                }
                log_sync(&format!("ingested {ok}/{seen}"));
                *state.source.lock().unwrap() = format!("{} · {ok} message(s) synced", cfg.user);
            }
            Err(e) => {
                log_sync(&format!("fetch FAILED after {ok} message(s): {e}"));
                *state.source.lock().unwrap() = format!("sync failed: {e}");
                *state.sync_error.lock().unwrap() = Some(friendly_sync_error(&format!("{e}")));
            }
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
            drain_actions(Arc::clone(&state), account, cfg.clone(), has_move).await;

            let since = inbox_id
                .and_then(|fid| {
                    state
                        .store
                        .lock()
                        .ok()
                        .and_then(|s| s.max_uid(fid).ok().flatten())
                })
                .unwrap_or(0);

            let mut fresh = 0usize;
            let polled = {
                let state = Arc::clone(&state);
                petrel_providers::imap::fetch_since_each(&cfg, "INBOX", since, |uid, flags, raw| {
                    let Ok(mut store) = state.store.lock() else {
                        return;
                    };
                    if let Ok(ingested) =
                        store.ingest_raw(&state.blobs, account, inbox_id, Some(uid), raw)
                    {
                        let _ = store.set_message_flags(ingested.message_id, flags);
                        fresh += 1;
                    }
                })
                .await
            };
            match polled {
                Ok(_) if fresh > 0 => {
                    log_sync(&format!("poll: {fresh} new message(s)"));
                    // The list watches this count, so bumping it is what makes
                    // new mail appear without the user doing anything.
                    state.seeded.fetch_add(fresh, Ordering::Relaxed);
                    *state.sync_error.lock().unwrap() = None;
                }
                Ok(_) => {}
                Err(e) => log_sync(&format!("poll failed: {e}")),
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

/// Tags for the rail. Comes from the account, not from whatever rows happen to
/// be loaded — a tag with no conversation in the current page still exists.
#[tauri::command]
fn list_tags(state: State<Arc<AppState>>) -> Result<Vec<TagSummary>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.first_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.tags_for_account(account).map_err(|e| e.to_string())
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
        .first_account()
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
    in_reply_to: Option<String>,
    references: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    use petrel_providers::smtp::{Outgoing, SendResult, SmtpConfig, send_tls};

    let cfg = imap_config_from_env().ok_or("no account is configured")?;
    let smtp = SmtpConfig::for_imap_host(&cfg.host, &cfg.user, &cfg.pass);
    let domain = cfg
        .user
        .split('@')
        .nth(1)
        .unwrap_or("localhost")
        .to_string();

    let msg = Outgoing {
        from_addr: cfg.user.clone(),
        from_name: String::new(),
        to,
        cc,
        subject,
        body_text: body,
        in_reply_to,
        references,
    };
    if msg.recipients().is_empty() {
        return Err("a message needs at least one recipient".into());
    }
    let (message_id, raw) = msg.render(&domain);

    let outcome = send_tls(&smtp, &msg, &raw).await;
    match outcome {
        SendResult::Committed { .. } => {}
        SendResult::RejectedPermanently { response } => {
            log_sync(&format!("send rejected: {response}"));
            return Err(format!("The server refused the message: {response}"));
        }
        SendResult::FailedBeforeCommit { stage, detail } => {
            log_sync(&format!("send failed at {stage}: {detail}"));
            return Err(format!("Could not send ({stage}): {detail}"));
        }
        SendResult::UnknownAfterTransmit { detail } => {
            // Spike S5's case: the body went, the acknowledgement did not. The
            // message may well have been delivered, so a retry could duplicate
            // it. Say so plainly rather than guessing either way.
            log_sync(&format!(
                "send outcome unknown: {detail} (message-id {message_id})"
            ));
            return Err(
                "The message may have been sent — the connection dropped before the server \
                 confirmed. Check your Sent folder before sending it again."
                    .into(),
            );
        }
    }

    // Filed second, and separately: a failure here has not lost the message.
    let sent_path = {
        let store = state.store.lock().map_err(|_| "store lock poisoned")?;
        let account = store
            .first_account()
            .map_err(|e| e.to_string())?
            .ok_or("no account")?;
        store
            .folder_for_role(account, "sent")
            .ok()
            .flatten()
            .and_then(|fid| store.folder_path(fid).ok().flatten())
    };
    if let Some(path) = sent_path {
        if let Err(e) = petrel_providers::imap::append_message(&cfg, &path, &raw).await {
            log_sync(&format!("sent, but could not file a copy in {path}: {e}"));
        }
    }
    log_sync(&format!("sent {message_id}"));
    Ok(message_id)
}

/// Folders for the move picker (V).
#[tauri::command]
fn list_folders(state: State<Arc<AppState>>) -> Result<Vec<FolderSummary>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let Some(account) = store.first_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.folders(account).map_err(|e| e.to_string())
}

/// Creates a folder the user named, or returns the one already there. The
/// picker offers this on the end of the same keystroke as choosing one.
#[tauri::command]
fn create_folder(path: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .first_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    store
        .ensure_named_folder(account, &path)
        .map_err(|e| e.to_string())
}

/// Creates a tag, or returns the one already there — same shape as folders.
#[tauri::command]
fn create_tag(name: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    let account = store
        .first_account()
        .map_err(|e| e.to_string())?
        .ok_or("no account")?;
    store
        .ensure_tag(account, &name, None)
        .map_err(|e| e.to_string())
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
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadListing>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.search_threads(&query, 200).map_err(|e| e.to_string())
}

/// Issues a one-message URL for the reading pane. The UI never receives the
/// body over IPC — bulk bytes go over the custom protocol, and the frame that
/// renders them has no IPC access at all.
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

  // Input reachability probe: reports the first of each kind of event to land,
  // and what was under the pointer. A webview that renders but never sees a
  // click looks identical to a frozen one, so this distinguishes them.
  var seen = {};
  function once(kind, extra) {
    if (seen[kind]) return;
    seen[kind] = 1;
    send({ kind: 'input', event: kind, detail: extra || null });
  }
  window.addEventListener('pointermove', function (e) {
    var el = e.target;
    once('pointermove', el ? (el.className || el.tagName) + '' : 'none');
  }, true);
  window.addEventListener('click', function (e) {
    var el = e.target;
    once('click', el ? (el.className || el.tagName) + '' : 'none');
  }, true);
  window.addEventListener('wheel', function () { once('wheel'); }, true);
  window.addEventListener('scroll', function (e) {
    var el = e.target;
    once('scroll', el && el.className ? el.className + '' : 'document');
  }, true);
  window.addEventListener('keydown', function (e) { once('keydown', e.key); }, true);

  // Focus and hit-testing, sampled repeatedly: distinguishes "the webview never
  // gets focus" from "something invisible is on top of the page".
  window.addEventListener('focus', function () { send({ kind: 'win', e: 'focus' }); }, true);
  window.addEventListener('blur', function () { send({ kind: 'win', e: 'blur' }); }, true);
  var ticks = 0;
  var beat = setInterval(function () {
    ticks++;
    var el = null;
    try { el = document.elementFromPoint(400, 400); } catch (e) {}
    send({
      kind: 'focus-probe',
      tick: ticks,
      hasFocus: document.hasFocus(),
      visibility: document.visibilityState,
      active: document.activeElement ? document.activeElement.tagName : null,
      at400x400: el ? (el.className || el.tagName) + '' : 'nothing',
      events: Object.keys(seen).join(',') || 'none'
    });
    // runs for the life of the window
  }, 3000);
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
        server_has_move: AtomicBool::new(false),
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

    match imap_config_from_env() {
        Some(cfg) => {
            eprintln!("[sync] account configured: {} @ {}", cfg.user, cfg.host);
            spawn_real_sync(state.clone(), account, cfg);
        }
        None => {
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
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            status,
            list_messages,
            list_threads,
            list_tags,
            thread_detail,
            triage,
            undo_triage,
            list_folders,
            create_folder,
            create_tag,
            send_message,
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
            message_view::handle(&request, &state.tokens, &state.blobs, lookup)
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
