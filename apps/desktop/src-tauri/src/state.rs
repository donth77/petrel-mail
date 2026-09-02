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
    /// Raised when local triage is waiting to reach the server. The drain
    /// worker sleeps on this rather than on a timer, so an archive reaches
    /// Gmail in about a second instead of whenever the next sync happens to
    /// come round — which, with IDLE holding a connection open, could be
    /// twenty minutes.
    pub(crate) drain_signal: Arc<tokio::sync::Notify>,
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
    /// When a sync cycle last completed clean, in ms. Zero until one has.
    /// The status bar ages this into words; a static "just now" was the
    /// previous implementation, and it was stuck by construction.
    pub(crate) last_sync_ms: std::sync::atomic::AtomicI64,
    /// When the user last asked the store for something, in ms. Backfill
    /// yields to this: history is the least urgent work in the program, and
    /// a stride that makes a click wait has its priorities inverted.
    pub(crate) ui_touch_ms: std::sync::atomic::AtomicI64,
    /// Whether the server supports UID MOVE, learned from the probe.
    pub(crate) server_has_move: AtomicBool,
    /// RFC 4315. Without it a message can be marked deleted but not expunged,
    /// because a bare EXPUNGE would take every other \\Deleted message with it.
    pub(crate) server_has_uidplus: AtomicBool,
    /// Whether this account's tags are Gmail labels rather than IMAP keywords.
    pub(crate) server_is_gmail: AtomicBool,
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

impl AppState {
    /// The outbox wake-ups for an account, made on first use. The send
    /// worker and the clock both ask, so whichever spawns first creates them.
    pub(crate) fn outbox_signals(&self, account: i64) -> Arc<OutboxSignals> {
        let mut all = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = all.iter().find(|s| s.account == account) {
            return Arc::clone(s);
        }
        let s = Arc::new(OutboxSignals {
            account,
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
