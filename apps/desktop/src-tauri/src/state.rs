//! What the whole app shares: the store, the blob store, and the flags the workers and commands coordinate through.

use crate::diag::log_sync;
use crate::message_view::ViewTokens;
use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

pub(crate) struct AppState {
    pub(crate) store: Mutex<Store>,
    pub(crate) blobs: BlobStore,
    pub(crate) seeding: AtomicBool,
    /// True when the window is showing synthetic mail because no account is
    /// configured. The UI needs to tell that apart from a first run: both have
    /// no account, but one has a mailbox to show and the other has onboarding
    /// to offer, and treating demo mode as a first run hid the demo entirely.
    pub(crate) demo: AtomicBool,
    pub(crate) seeded: AtomicUsize,
    /// Last successful `status` count. Lock failure must not fall back to
    /// `seeded` — that counter climbs during backfill and made the UI think
    /// the mailbox size was jumping, so it refetched the list on every poll.
    pub(crate) status_count: AtomicUsize,
    pub(crate) source: Mutex<String>,
    /// Set when a sync fails. Separate from `source` because a failure has to
    /// reach the screen, and `source` is a label the UI is free to ignore —
    /// which is exactly what it did, leaving a failed login looking like an
    /// empty mailbox.
    pub(crate) sync_error: Mutex<Option<String>>,
    /// One stop switch per account, flipped when the account is removed so
    /// its workers stand down instead of carrying on against a server the
    /// app no longer owns — or against a new account that inherited the same
    /// id, which is what remove-and-re-add hands out. `stop_signal` gives a
    /// worker its receiver; `stop_workers` flips the switch and *keeps* it,
    /// so a receiver taken afterwards reads stopped too. Only `reset_workers`,
    /// called when an account is set up, puts a fresh switch in its place.
    pub(crate) stops: Mutex<std::collections::HashMap<i64, tokio::sync::watch::Sender<bool>>>,
    /// Paths the OS file dialogs handed back this session. A command that
    /// takes a path from the window accepts only one of these, one under the
    /// staging directory, or one already on the draft being edited — the
    /// window never gets to name a file on disk by itself.
    pub(crate) picked: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// What each account's server can do, from that account's own probe. One
    /// set of flags for the whole app used to be written by whichever account
    /// probed last, so with Gmail beside Dovecot half of launches drained one
    /// account's changes with the other's commands.
    pub(crate) caps: Mutex<std::collections::HashMap<i64, ServerCaps>>,
    /// One pair of outbox wake-ups per account, registered by the account's
    /// sync when it starts. A single shared `Notify` looked simpler and was
    /// wrong with two accounts: `notify_one` wakes whichever worker is first
    /// in the queue, so a send queued on the other account waited for its
    /// clock to nag. Per account, a wake is a permit for exactly the worker
    /// that owns the row, and a worker mid-send finds it waiting when it
    /// comes back.
    pub(crate) outbox: Mutex<Vec<Arc<OutboxSignals>>>,
    /// One drain at a time. Two overlapping passes would both read the same
    /// queued rows and deliver each change twice.
    pub(crate) draining: AtomicBool,
    /// Drafts edited since their last push to the server, for the 30-second
    /// debounce. A draft in here has exactly one push task sleeping on it.
    pub(crate) draft_dirty: Mutex<std::collections::HashSet<i64>>,
    /// Arrivals a rule marked notify-anyway, waiting for the next status
    /// poll to carry them to the announcer. Drained on read: each is said
    /// once.
    pub(crate) pending_notify: Mutex<Vec<(String, String)>>,
    /// Things a worker needs the person to know, by key, waiting for the
    /// next status poll. The window owns the words: a worker has no
    /// language of its own, and on two of the three platforms no way to
    /// post a notification either. Drained on read; each is said once.
    pub(crate) pending_alerts: Mutex<Vec<String>>,
    /// When a sync cycle last completed clean, in ms. Zero until one has.
    /// The status bar ages this into words; a static "just now" was the
    /// previous implementation, and it was stuck by construction.
    pub(crate) last_sync_ms: std::sync::atomic::AtomicI64,
    /// When the user last asked the store for something, in ms. Backfill
    /// yields to this: history is the least urgent work in the program, and
    /// a stride that makes a click wait has its priorities inverted.
    pub(crate) ui_touch_ms: std::sync::atomic::AtomicI64,
    /// How much mail the server says it holds, across the folders we sync.
    ///
    /// The denominator of the coverage line, and the reason it exists: a client
    /// that quietly returns three results out of a possible ten teaches you not
    /// to trust its search. Zero until a sync has asked.
    pub(crate) server_total: std::sync::atomic::AtomicUsize,
    /// Messages the user asked to see this once. Deliberately not persisted:
    /// "show images" is a decision about one message on one occasion, and a
    /// version of it that outlived the session would be trust nobody granted.
    pub(crate) shown_once: Mutex<std::collections::HashSet<i64>>,
    pub(crate) tokens: Arc<ViewTokens>,
    pub(crate) account_id: i64,
    pub(crate) data_dir: String,
}

/// The wake-ups for one account's outbox: its send worker and its clock.
pub(crate) struct OutboxSignals {
    pub(crate) account: i64,
    /// Raised when local triage on this account is waiting to reach the
    /// server. The drain worker sleeps on this rather than on a timer, so an
    /// archive reaches the server in about a second rather than at the next
    /// sync. One per account: a single shared signal woke whichever worker
    /// had waited longest, and the account with the change waited for its
    /// next IDLE wake or the five-minute sweep.
    pub(crate) drain: tokio::sync::Notify,
    /// Raised when a queued send should go now. Send now, the outbox clock,
    /// and a scheduled send wake this; triage does not. Sharing the drain
    /// signal put SMTP behind every pending IMAP STORE/MOVE — observed live
    /// as two minutes of "still in the outbox" while fifteen actions drained.
    pub(crate) send: tokio::sync::Notify,
    /// Aborts the outbox clock's sleep so it re-reads the next due time.
    /// A new schedule used to land while the clock was in a 60s empty-outbox
    /// nap, so the undo window expired and nothing sent until someone pressed
    /// Send now.
    pub(crate) clock: tokio::sync::Notify,
}

/// What one account's server advertised when it was probed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ServerCaps {
    pub(crate) has_move: bool,
    pub(crate) has_uidplus: bool,
    pub(crate) is_gmail: bool,
}

impl AppState {
    /// Local triage on this account is waiting to reach the server.
    pub(crate) fn nudge_drain(&self, account: i64) {
        self.outbox_signals(account).drain.notify_one();
    }

    /// The receiver a worker for this account watches. Made on first use, so
    /// whichever worker spawns first creates the switch; a receiver reads
    /// `true` once `stop_workers` has run — whether it was taken before or
    /// after — and `stopped` resolves as soon as it does.
    pub(crate) fn stop_signal(&self, account: i64) -> tokio::sync::watch::Receiver<bool> {
        let mut all = self.stops.lock().unwrap_or_else(|p| p.into_inner());
        all.entry(account)
            .or_insert_with(|| tokio::sync::watch::channel(false).0)
            .subscribe()
    }

    /// Flips the account's switch, and leaves it flipped.
    ///
    /// The switch used to be forgotten here so that a re-added account got a
    /// fresh one — which meant a worker that asked for its receiver *after*
    /// the removal got a fresh one too. The first sync pass takes minutes,
    /// and its watchers and backfill only asked at the end of it, so an
    /// account removed during that pass kept syncing, and the next account
    /// to be set up inherited the id and the mail. Stopped stays stopped
    /// until `reset_workers` says otherwise. Returns whether any worker had
    /// ever asked about this account.
    pub(crate) fn stop_workers(&self, account: i64) -> bool {
        let mut all = self.stops.lock().unwrap_or_else(|p| p.into_inner());
        let known = all.contains_key(&account);
        let tx = all
            .entry(account)
            .or_insert_with(|| tokio::sync::watch::channel(false).0);
        // `send` fails when nothing is subscribed yet — and *leaves the value
        // alone*, which is exactly the case that matters: an account removed
        // before its workers ever asked for the switch would have been given
        // a false one. `send_replace` writes the value either way.
        tx.send_replace(true);
        known
    }

    /// A fresh switch for an account being set up, so its workers start
    /// clean however its id was used before. Whatever was watching the old
    /// switch still reads stopped: the shared value never goes back to false.
    pub(crate) fn reset_workers(&self, account: i64) {
        self.stops
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account, tokio::sync::watch::channel(false).0);
    }

    /// Queues an alert for the window to say, once.
    pub(crate) fn raise_alert(&self, key: &str) {
        self.pending_alerts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(key.to_string());
    }

    /// Records paths the OS dialogs handed back, so commands can accept them.
    pub(crate) fn remember_paths(&self, paths: &[std::path::PathBuf]) {
        let mut picked = self.picked.lock().unwrap_or_else(|p| p.into_inner());
        picked.extend(paths.iter().cloned());
    }

    /// A path the window named, checked against what it was ever given.
    ///
    /// `on_draft` is the attachment list already stored on the draft being
    /// edited, which came through this same check when it was saved.
    pub(crate) fn vetted_path(
        &self,
        path: &str,
        on_draft: &[String],
    ) -> Result<std::path::PathBuf, String> {
        let picked = self.picked.lock().unwrap_or_else(|p| p.into_inner());
        accept_path(
            &picked,
            &crate::diag::data_dir().join("staged"),
            on_draft,
            path,
        )
    }

    /// What this account's server can do; nothing, until its probe answers.
    pub(crate) fn caps(&self, account: i64) -> ServerCaps {
        self.caps
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&account)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn set_caps(&self, account: i64, caps: ServerCaps) {
        self.caps
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account, caps);
    }

    /// The outbox wake-ups for an account, made on first use. The send
    /// worker and the clock both ask, so whichever spawns first creates them.
    pub(crate) fn outbox_signals(&self, account: i64) -> Arc<OutboxSignals> {
        let mut all = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = all.iter().find(|s| s.account == account) {
            return Arc::clone(s);
        }
        let s = Arc::new(OutboxSignals {
            account,
            drain: tokio::sync::Notify::new(),
            send: tokio::sync::Notify::new(),
            clock: tokio::sync::Notify::new(),
        });
        all.push(Arc::clone(&s));
        s
    }

    /// A send was queued or marked due. Wake every account's worker, and its
    /// clock so it sleeps until the new time rather than finishing an
    /// empty-outbox nap. Every account rather than one: the callers hold a
    /// draft id, not an account, and a pass over an empty queue is one SELECT.
    pub(crate) fn wake_send(&self) {
        for s in self.outbox.lock().unwrap_or_else(|p| p.into_inner()).iter() {
            s.send.notify_one();
            s.clock.notify_one();
        }
    }

    /// The drain or the sync loop has finished with the server: anything that
    /// became due meanwhile goes out on this account's worker.
    pub(crate) fn nudge_send(&self, account: i64) {
        let all = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = all.iter().find(|s| s.account == account) {
            s.send.notify_one();
        }
    }

    /// The store, or the one error every command reports when the lock is
    /// poisoned — a panic on another thread while it held the store.
    pub(crate) fn store(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        self.store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
    }
}

/// Resolves once the account has been told to stop — at once, if it already
/// has. Every worker's wait is a `select!` against this, so a removal ends a
/// twenty-minute IDLE, a backfill nap or a fetch in progress rather than
/// waiting for it.
pub(crate) async fn stopped(stop: &mut tokio::sync::watch::Receiver<bool>) {
    while !*stop.borrow() {
        // The sender going away is a stop too: it means the account's switch
        // was replaced, and whoever holds this receiver belongs to the old one.
        if stop.changed().await.is_err() {
            return;
        }
    }
}

/// Runs `work` unless the account is stopped first. `None` means it was,
/// and the work was abandoned wherever it had got to.
pub(crate) async fn unless_stopped<T>(
    stop: &mut tokio::sync::watch::Receiver<bool>,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    if *stop.borrow() {
        return None;
    }
    tokio::select! {
        v = work => Some(v),
        _ = stopped(stop) => None,
    }
}

/// Whether a path the window named may be read or written.
///
/// Three ways in, and no fourth: the OS dialog produced it this session, it
/// lives in the staging directory Petrel itself writes dropped files to, or
/// it is already on the draft being edited. Message content is kept out of
/// the app by design; this is what keeps a compromised page from turning
/// "attach this file" into "mail any file on the disk".
pub(crate) fn accept_path(
    picked: &std::collections::HashSet<std::path::PathBuf>,
    staged_dir: &std::path::Path,
    on_draft: &[String],
    path: &str,
) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, Path, PathBuf};
    let candidate = PathBuf::from(path);
    if picked.contains(&candidate) || on_draft.iter().any(|p| p == path) {
        return Ok(candidate);
    }
    // Lexically under the staging directory, with nothing that climbs back
    // out of it: `staged/../petrel.db` starts with the right prefix and
    // names the database.
    let climbs = candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir));
    if !climbs && candidate.starts_with(staged_dir) && Path::new(path) != staged_dir {
        return Ok(candidate);
    }
    Err("that file was not chosen through Petrel".into())
}

/// The account on screen, as commands need it: an id, or the error that
/// there is none yet.
pub(crate) fn active_account(store: &Store) -> Result<i64, String> {
    store
        .active_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no account".to_string())
}

/// Logs any instrumented command that took longer than feels instant.
///
/// The performance claim is not allowed to be a feeling: every hot command
/// carries one of these, and the log holds the distribution — slow calls
/// name themselves, silence means everything ran under the threshold.
pub(crate) struct Timed(&'static str, std::time::Instant);

impl Timed {
    pub(crate) fn new(name: &'static str) -> Self {
        Timed(name, std::time::Instant::now())
    }
}

impl Drop for Timed {
    fn drop(&mut self) {
        let ms = self.1.elapsed().as_millis();
        if ms > 50 {
            log_sync(&format!("SLOW {}: {ms}ms", self.0));
        }
    }
}

/// Marks "the user is here, working" — called by the interactive commands.
pub(crate) fn note_ui_touch(state: &AppState) {
    state
        .ui_touch_ms
        .store(now_ms(), std::sync::atomic::Ordering::Relaxed);
}

/// Whether the user asked for something in the last `within_ms`.
pub(crate) fn ui_recently_active(state: &AppState, within_ms: i64) -> bool {
    now_ms() - state.ui_touch_ms.load(std::sync::atomic::Ordering::Relaxed) < within_ms
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A state over an in-memory store, for tests of the coordination it holds.
#[cfg(test)]
pub(crate) fn test_state(dir: &std::path::Path) -> Arc<AppState> {
    let store = Store::open_in_memory().expect("store");
    let account = store.ensure_test_account().expect("account");
    Arc::new(AppState {
        store: Mutex::new(store),
        blobs: BlobStore::open(&dir.join("blobs")).expect("blobs"),
        seeding: AtomicBool::new(false),
        demo: AtomicBool::new(false),
        seeded: AtomicUsize::new(0),
        status_count: AtomicUsize::new(0),
        source: Mutex::new(String::new()),
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
    })
}

#[cfg(test)]
mod worker_switch_tests {
    use super::*;
    use std::collections::HashMap;

    /// The switch semantics every worker relies on. A receiver taken before
    /// the account is removed reads true afterwards; so does one taken for
    /// the same id *after* the removal — a worker that only asks at the end
    /// of a long first pass must not be handed a fresh switch. Only setting
    /// the account up again starts clean.
    #[test]
    fn a_stopped_account_stays_stopped_until_it_is_set_up_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(dir.path());

        let early = state.stop_signal(7);
        assert!(!*early.borrow());
        assert!(state.stop_workers(7), "a switch existed to flip");
        assert!(
            *early.borrow(),
            "the removed account's workers see the stop"
        );

        // The bug: this receiver used to be a fresh, un-flipped switch.
        let late = state.stop_signal(7);
        assert!(
            *late.borrow(),
            "a worker asking after the removal must read stopped too"
        );
        // And an id nobody has asked about yet is stopped the moment it is
        // removed, so nothing spawned for it later can run.
        assert!(!state.stop_workers(8));
        assert!(*state.stop_signal(8).borrow());

        // Setting the account up again is the one thing that resets it.
        state.reset_workers(7);
        let fresh = state.stop_signal(7);
        assert!(
            !*fresh.borrow(),
            "a re-added account under the same id starts clean"
        );
        assert!(
            *late.borrow(),
            "the old workers keep reading stopped; the new switch is not theirs"
        );
    }

    /// `stopped` is the wait every worker selects against: it resolves at
    /// once for a switch already flipped, and when its switch is replaced.
    #[test]
    fn the_stop_wait_resolves_at_once_for_an_account_already_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(dir.path());
        state.stop_workers(3);
        let mut rx = state.stop_signal(3);
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), stopped(&mut rx))
                .await
                .expect("resolves without waiting for a change");
            // Nothing runs on a stopped account.
            let ran = unless_stopped(&mut rx, async { 1 }).await;
            assert_eq!(ran, None);
        });
        // A replaced switch ends the old workers' wait too.
        let mut old = state.stop_signal(4);
        state.reset_workers(4);
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), stopped(&mut old))
                .await
                .expect("the old switch going away is a stop");
        });
    }

    /// A path reaches a command only if Petrel itself produced it.
    #[test]
    fn only_paths_petrel_handed_out_are_accepted() {
        use std::path::PathBuf;
        let staged = PathBuf::from("/data/Petrel/staged");
        let mut picked: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        picked.insert(PathBuf::from("/Users/me/Documents/report.pdf"));
        let on_draft = vec!["/Users/me/Pictures/old.png".to_string()];

        let ok = |p: &str| accept_path(&picked, &staged, &on_draft, p).is_ok();
        assert!(ok("/Users/me/Documents/report.pdf"), "picked this session");
        assert!(ok("/data/Petrel/staged/1-dropped.txt"), "staged by a drop");
        assert!(ok("/Users/me/Pictures/old.png"), "already on the draft");

        assert!(!ok("/etc/passwd"));
        assert!(!ok("/Users/me/.ssh/id_rsa"));
        assert!(
            !ok("/data/Petrel/staged/../petrel.db"),
            "climbing out of staged"
        );
        assert!(!ok("/data/Petrel/staged"), "the directory itself");
        assert!(
            !ok("/data/Petrel/stagedX/x"),
            "a sibling with the same prefix"
        );
        assert!(!ok(""));
    }

    #[test]
    fn capabilities_are_per_account_and_empty_until_probed() {
        let caps: Mutex<HashMap<i64, ServerCaps>> = Mutex::new(HashMap::new());
        let mut all = caps.lock().unwrap();
        all.insert(
            1,
            ServerCaps {
                has_move: true,
                has_uidplus: true,
                is_gmail: true,
            },
        );
        assert!(all.get(&1).copied().unwrap_or_default().is_gmail);
        assert!(!all.get(&2).copied().unwrap_or_default().is_gmail);
    }
}
