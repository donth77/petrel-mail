//! What the whole app shares: the store, the blob store, and the flags the workers and commands coordinate through.

use crate::diag::log_sync;
use crate::message_view::ViewTokens;
use petrel_engine::blob::BlobStore;
use petrel_engine::store::{
    Correspondent, CorrespondentStats, Store, WalCheckpoint, merge_correspondent_stats,
    rank_correspondents,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, TryLockError};

/// Correspondent totals for address completion. Process memory only, empty
/// at every launch. Writing them to disk would leave a deleted message, or
/// one filed as spam, as a suggestion until the next start.
///
/// `after_id` is the highest `messages.id` already read. The next warm adds
/// only rows past that watermark.
struct AccountAddressStats {
    stats: Vec<CorrespondentStats>,
    after_id: i64,
    own_email: String,
}

/// One build per account. The done flag is a `Mutex` because the `Arc` is
/// shared: a plain `bool` cannot be written by the other holder, and waiting
/// on the cache lock would keep the builder from publishing its result.
/// Waiters block on this lock. The cache is not held while SQL runs.
struct AddressBuild {
    done: Mutex<bool>,
    wait: Arc<Condvar>,
}

/// Per-account totals, and the build that is running now. The fields are
/// visible inside the crate; what they hold stays in this file. `lib.rs`
/// never names the type. It only calls the function that makes the slot.
#[derive(Default)]
pub(crate) struct AddressCompletionCache {
    accounts: HashMap<i64, AccountAddressStats>,
    building: HashMap<i64, Arc<AddressBuild>>,
}

/// How a keystroke and a warm share one account's build, so the two do
/// not both scan.
enum AddressSlot {
    /// Totals are already there and nobody is updating them. The caller
    /// filters in memory.
    Ready,
    /// This call does the read. `Some` is the incremental watermark;
    /// `None` reads every row.
    Started(Arc<AddressBuild>, Option<i64>),
    /// A build is already running. Do not start a scan; wait for it.
    Waiting(Arc<AddressBuild>),
}

pub(crate) struct AppState {
    pub(crate) store: Mutex<Store>,
    /// Extra connections for SELECTs. WAL already allows them to run during
    /// a write. Two share listing and search. Empty stand-ins when
    /// `readers_live` is false (tests, after a wipe).
    pub(crate) reads: [Mutex<Store>; 2],
    /// Sidebar and footer counts. Those queries take seconds on a large
    /// mailbox; they used to sit in `reads` and a search then waited
    /// behind them for a minute.
    pub(crate) read_counts: Mutex<Store>,
    /// One more connection, only for opening a message. Recounts keep the
    /// pool busy; the conversation index has its own. The body URL must
    /// not wait for either.
    pub(crate) read_open: Mutex<Store>,
    /// One more connection, only for the conversation index. Recounts
    /// keep the pool busy; older cards must not wait behind them.
    pub(crate) read_index: Mutex<Store>,
    pub(crate) readers_live: AtomicBool,
    pub(crate) blobs: BlobStore,
    /// Accounts whose first pass is running, each with the mark that began
    /// it. See `mark_seeding` in signin.rs.
    pub(crate) seeding: Mutex<HashMap<i64, u64>>,
    pub(crate) seeding_marks: std::sync::atomic::AtomicU64,
    /// Accounts Petrel cannot sign in to, why, and since when (ms). See
    /// signin.rs: a missing password waits for one, and a refused one is
    /// asked about once an hour rather than every couple of minutes.
    pub(crate) signin: Mutex<HashMap<i64, (crate::signin::SignIn, i64)>>,
    /// Bumped on every change to `signin`, so a worker standing down can
    /// wait for the change rather than poll for it.
    pub(crate) signin_changed: tokio::sync::watch::Sender<u64>,
    /// Work an account could not do while signed out: draft pushes, and the
    /// removal of sent or discarded drafts' server copies. See signin.rs.
    pub(crate) held: Mutex<HashMap<i64, crate::signin::Held>>,
    /// One change to an account's sign-in at a time (`account_turn`).
    pub(crate) turns: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
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
    /// Accounts whose folders a survey this session has listed and stored:
    /// which mailbox is the archive, and which of the server's mailboxes are
    /// views of everything rather than places. The launch survey marks them,
    /// and so does any later one, once what it found is in the store.
    pub(crate) surveyed: Mutex<std::collections::HashSet<i64>>,
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
    /// Held while a re-extraction walks the store.
    ///
    /// One walk, however many accounts are signed in. Which extraction a
    /// message was indexed by is a property of the store, not of an account,
    /// and the pass is driven by a single cursor — so a second walker does not
    /// repeat the first one's rows, it interleaves with them. Both would then
    /// reach the end, bump the generation, and checkpoint, for twice the lock
    /// pressure and two list reloads. The first account to get there does the
    /// work and the others leave it alone.
    pub(crate) reindexing: AtomicBool,
    /// Drafts edited since their last push to the server, for the 30-second
    /// debounce. A draft in here has exactly one push task sleeping on it.
    pub(crate) draft_dirty: Mutex<std::collections::HashSet<i64>>,
    /// On-open folder fetches already running, keyed by the view
    /// (`sent`, `folder:3`). Opening the same mailbox twice must not
    /// stack IMAP sessions on it. Different mailboxes may overlap.
    pub(crate) folder_sync_inflight: Mutex<HashSet<String>>,
    /// When each on-open fetch last finished, by the same key. The inflight
    /// set stops two fetches at once; this stops a login per click when
    /// somebody flicks between Sent and Drafts.
    pub(crate) folder_synced_at: Mutex<HashMap<String, std::time::Instant>>,
    /// Arrivals a rule marked notify-anyway, waiting for the next status
    /// poll to carry them to the announcer. Drained on read: each is said
    /// once.
    pub(crate) pending_notify: Mutex<Vec<(String, String)>>,
    /// New mail in the inbox of an account that is not on screen, waiting
    /// for the next status poll to carry it to the announcer: the window
    /// watches only the inbox of the account it shows. Drained on read;
    /// each is said once. See `announce_elsewhere`.
    pub(crate) pending_elsewhere: Mutex<Vec<Elsewhere>>,
    /// Things a worker needs the person to know, by key, waiting for the
    /// next status poll. The window owns the words: a worker has no
    /// language of its own, and on two of the three platforms no way to
    /// post a notification either. Drained on read; each is said once.
    pub(crate) pending_alerts: Mutex<Vec<String>>,
    /// When a sync cycle last completed clean, in ms. Zero until one has.
    /// The status bar ages this into words; a static "just now" was the
    /// previous implementation, and it was stuck by construction.
    pub(crate) last_sync_ms: std::sync::atomic::AtomicI64,
    /// Bumped when a re-extraction has rewritten stored text. The status
    /// poll carries it so the window reloads the list; otherwise the
    /// heading keeps the subject it loaded at launch.
    pub(crate) extraction_gen: std::sync::atomic::AtomicI64,
    /// Bumped when a sync changed where mail sits or how it is flagged,
    /// whether or not the amount of mail changed. The list reloaded on the
    /// message count alone, and a move made in another client changes no
    /// count: the mail is the same mail, in a different folder. So the
    /// folder on screen kept showing what had left it, and never showed
    /// what had arrived.
    pub(crate) mail_gen: std::sync::atomic::AtomicI64,
    /// The mailbox on screen, with the account it belongs to. Each
    /// account's folder watch follows it; `None` until the window says.
    pub(crate) open_view: tokio::sync::watch::Sender<Option<(i64, String)>>,
    /// `(MESSAGES, UIDNEXT)` from the last STATUS each folder answered,
    /// by folder id — the baseline that lets a pass notice mail that left
    /// without asking the server for every UID. In memory on purpose: the
    /// first pass of a launch has none and falls back to comparing counts.
    pub(crate) folder_seen: Mutex<HashMap<i64, (u32, u32)>>,
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
    /// Correspondent stats for address completion, per account, in this
    /// process only. Typing ranks the vec instead of scanning the mailbox,
    /// and a second open of the composer must not load it all again.
    pub(crate) address_completion: Mutex<AddressCompletionCache>,
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

/// One message new to the inbox of an account that is not on screen: who it
/// is from and what it is about, for the window to say, and whose it is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Elsewhere {
    pub(crate) account: i64,
    pub(crate) who: String,
    pub(crate) subject: String,
}

/// What one account's server advertised when it was probed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ServerCaps {
    pub(crate) has_move: bool,
    pub(crate) has_uidplus: bool,
    pub(crate) is_gmail: bool,
    pub(crate) has_idle: bool,
    /// A probe has answered this session. Until one does, every `false`
    /// above means "not known yet", not "the server cannot" — a launch with
    /// no network never ran its probe.
    pub(crate) known: bool,
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

    /// Whether a folder survey this session has stored what it found for the
    /// account: which mailbox is the archive, and which of the server's
    /// mailboxes are views of everything rather than places.
    pub(crate) fn surveyed(&self, account: i64) -> bool {
        self.surveyed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&account)
    }

    /// Records a survey as stored. After the store has its findings, not
    /// before: an Archive opened in between would sync on the old marks. Not
    /// only at launch either: a launch that found no network never surveyed,
    /// and the next survey that gets through is as good.
    pub(crate) fn mark_surveyed(&self, account: i64) {
        self.surveyed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account);
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

    /// Truncates the WAL if no listing is mid-SELECT.
    ///
    /// Other connections holding a snapshot make TRUNCATE return busy.
    /// The reader locks are taken first so those connections are idle.
    /// A listing already running is left alone; the caller retries.
    pub(crate) fn checkpoint_wal_truncate(&self) -> Result<WalCheckpoint, String> {
        let mut held = Vec::new();
        if self.readers_live.load(Ordering::Relaxed) {
            for slot in [
                &self.reads[0],
                &self.reads[1],
                &self.read_counts,
                &self.read_open,
                &self.read_index,
            ] {
                match slot.try_lock() {
                    Ok(g) => held.push(g),
                    Err(TryLockError::Poisoned(p)) => held.push(p.into_inner()),
                    Err(TryLockError::WouldBlock) => {
                        return Ok(WalCheckpoint {
                            busy: true,
                            log: 0,
                            checkpointed: 0,
                        });
                    }
                }
            }
        }
        let store = match self.store.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Ok(WalCheckpoint {
                    busy: true,
                    log: 0,
                    checkpointed: 0,
                });
            }
        };
        let _held = held;
        store.checkpoint_wal().map_err(|e| e.to_string())
    }

    /// A connection that may read while the writer is busy.
    ///
    /// Falls back to the write store when no secondary was opened — in-memory
    /// tests have no file a second connection can attach to.
    pub(crate) fn store_read(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        if !self.readers_live.load(Ordering::Relaxed) {
            return self.store();
        }
        for slot in &self.reads {
            match slot.try_lock() {
                Ok(g) => return Ok(g),
                Err(TryLockError::Poisoned(p)) => return Ok(p.into_inner()),
                Err(TryLockError::WouldBlock) => {}
            }
        }
        self.reads[0]
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
    }

    /// Counts only. A listing or a search must not wait on this.
    pub(crate) fn store_read_counts(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        if !self.readers_live.load(Ordering::Relaxed) {
            return self.store();
        }
        self.read_counts
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
    }

    /// The connection that issues a body URL and serves the frame.
    ///
    /// Separate from [`Self::store_read`] and [`Self::store_read_index`] so a
    /// recount or a long index cannot hold the reader while the pane waits
    /// for one hash.
    pub(crate) fn store_read_open(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        if !self.readers_live.load(Ordering::Relaxed) {
            return self.store();
        }
        self.read_open
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
    }

    /// The connection that lists a conversation's cards.
    ///
    /// Separate from the listing pool so a recount does not hold the index,
    /// and from [`Self::store_read_open`] so a long index does not hold the
    /// body URL.
    pub(crate) fn store_read_index(&self) -> Result<std::sync::MutexGuard<'_, Store>, String> {
        if !self.readers_live.load(Ordering::Relaxed) {
            return self.store();
        }
        self.read_index
            .lock()
            .map_err(|_| "store lock poisoned".to_string())
    }

    /// Drops file-backed readers so the mailbox can be moved aside.
    /// Further reads use the write store, which the wipe replaces with memory.
    pub(crate) fn drop_readers(&self) {
        self.readers_live.store(false, Ordering::Relaxed);
        for slot in &self.reads {
            if let Ok(mut g) = slot.lock()
                && let Ok(empty) = Store::open_in_memory()
            {
                *g = empty;
            }
        }
        if let Ok(mut g) = self.read_open.lock()
            && let Ok(empty) = Store::open_in_memory()
        {
            *g = empty;
        }
        if let Ok(mut g) = self.read_index.lock()
            && let Ok(empty) = Store::open_in_memory()
        {
            *g = empty;
        }
        if let Ok(mut g) = self.read_counts.lock()
            && let Ok(empty) = Store::open_in_memory()
        {
            *g = empty;
        }
    }

    /// The address-completion slot. `lib.rs` sets one up without naming
    /// the type; it only calls this.
    pub(crate) fn address_completion_slot() -> Mutex<AddressCompletionCache> {
        Mutex::new(AddressCompletionCache::default())
    }

    /// Ranks this account's totals against the prefix. SQL does not run under
    /// this lock: the build publishes through it, and a query here would hold
    /// that publish off while a keystroke waits.
    /// Nothing stored yet is an empty list. Another account's rows are not
    /// this one's suggestions.
    fn address_completion_rank(
        &self,
        account: i64,
        prefix: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<Correspondent>, String> {
        let cache = self
            .address_completion
            .lock()
            .map_err(|_| "address cache lock poisoned".to_string())?;
        let Some(entry) = cache.accounts.get(&account) else {
            return Ok(Vec::new());
        };
        Ok(rank_correspondents(
            &entry.stats,
            prefix,
            now_ms,
            &entry.own_email,
            limit,
        ))
    }

    /// One build for this account, or the waiter if one is already running.
    /// A second scan of the same mailbox is what this exists to avoid.
    /// Totals already stored are not reread unless this is a warm, and a
    /// warm reads only past the watermark.
    fn begin_address_completion(
        &self,
        account: i64,
        incremental: bool,
    ) -> Result<AddressSlot, String> {
        let mut cache = self
            .address_completion
            .lock()
            .map_err(|_| "address cache lock poisoned".to_string())?;
        if let Some(existing) = cache.building.get(&account) {
            return Ok(AddressSlot::Waiting(Arc::clone(existing)));
        }
        if let Some(entry) = cache.accounts.get(&account) {
            if !incremental {
                return Ok(AddressSlot::Ready);
            }
            let after_id = entry.after_id;
            let build = Arc::new(Self::new_address_build());
            cache.building.insert(account, Arc::clone(&build));
            return Ok(AddressSlot::Started(build, Some(after_id)));
        }
        let build = Arc::new(Self::new_address_build());
        cache.building.insert(account, Arc::clone(&build));
        Ok(AddressSlot::Started(build, None))
    }

    fn new_address_build() -> AddressBuild {
        AddressBuild {
            done: Mutex::new(false),
            wait: Arc::new(Condvar::new()),
        }
    }

    /// Stores a full snapshot for this account. The address stays as read.
    /// Ranking lowercases when it compares, and a folded copy here would be
    /// a second one to keep in step.
    /// Only this build's mark is cleared. It comes off before waiters wake,
    /// and a build that has taken the slot since must keep it.
    fn publish_address_completion(
        &self,
        account: i64,
        build: &Arc<AddressBuild>,
        stats: Vec<CorrespondentStats>,
        after_id: i64,
        own_email: String,
    ) -> Result<(), String> {
        let mut cache = self
            .address_completion
            .lock()
            .map_err(|_| "address cache lock poisoned".to_string())?;
        cache.accounts.insert(
            account,
            AccountAddressStats {
                stats,
                after_id,
                own_email,
            },
        );
        Self::clear_own_build(&mut cache, account, build);
        Ok(())
    }

    /// Folds an incremental read into the totals already stored. The watermark
    /// is the snapshot's highest message id, not the highest id in the delta:
    /// deleted mail and spam move that id without appearing there, and the
    /// next warm would reread them.
    /// A blank address leaves the one already stored. Wiping it would start
    /// offering the account's own address.
    fn merge_published_stats(
        &self,
        account: i64,
        build: &Arc<AddressBuild>,
        delta: Vec<CorrespondentStats>,
        watermark: i64,
        email: String,
    ) -> Result<(), String> {
        let mut cache = self
            .address_completion
            .lock()
            .map_err(|_| "address cache lock poisoned".to_string())?;
        if let Some(entry) = cache.accounts.get_mut(&account) {
            merge_correspondent_stats(&mut entry.stats, delta);
            entry.after_id = watermark;
            if !email.trim().is_empty() {
                entry.own_email = email;
            }
        }
        Self::clear_own_build(&mut cache, account, build);
        Ok(())
    }

    fn clear_own_build(
        cache: &mut AddressCompletionCache,
        account: i64,
        build: &Arc<AddressBuild>,
    ) {
        if cache
            .building
            .get(&account)
            .is_some_and(|current| Arc::ptr_eq(current, build))
        {
            cache.building.remove(&account);
        }
    }

    /// Wakes whoever is blocked on this build and clears its mark, even when
    /// the read failed. A mark left set would pin the account to a build
    /// that never finishes. This does not wait. The caller that started the
    /// build is the one with the result.
    fn finish_address_build(&self, account: i64, build: &Arc<AddressBuild>) {
        match self.address_completion.lock() {
            Ok(mut cache) => Self::clear_own_build(&mut cache, account, build),
            Err(poisoned) => {
                let mut cache = poisoned.into_inner();
                Self::clear_own_build(&mut cache, account, build);
            }
        }
        let mut done = build.done.lock().unwrap_or_else(|p| p.into_inner());
        *done = true;
        build.wait.notify_all();
    }

    fn wait_address_build(build: &AddressBuild) {
        let mut done = build.done.lock().unwrap_or_else(|p| p.into_inner());
        while !*done {
            done = build.wait.wait(done).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// One snapshot on a read connection, and not under the cache lock.
    /// That lock covers every account, so a scan held there would stall
    /// everyone else's completion. The write store stays free for the same
    /// reason: this read can be the whole mailbox.
    fn load_address_completion(
        &self,
        account: i64,
        since: Option<i64>,
    ) -> Result<(Vec<CorrespondentStats>, i64, String), String> {
        let store = self.store_read()?;
        store
            .read_correspondent_cache(account, since)
            .map_err(|e| e.to_string())
    }

    fn store_loaded_address_completion(
        &self,
        account: i64,
        build: &Arc<AddressBuild>,
        since: Option<i64>,
        loaded: (Vec<CorrespondentStats>, i64, String),
    ) -> Result<(), String> {
        let (stats, watermark, email) = loaded;
        if since.is_some() {
            self.merge_published_stats(account, build, stats, watermark, email)
        } else {
            self.publish_address_completion(account, build, stats, watermark, email)
        }
    }

    /// Suggestions for a keystroke. Stored totals are filtered in memory.
    /// The first miss reads the mailbox once; a keystroke that arrives during
    /// it waits, instead of scanning again.
    /// A blank prefix is not a search and must not start that read.
    /// The starter does not wait: it is what marks the build done, so waiting
    /// would deadlock. Finish runs before an error returns, or a failed read
    /// would leave the others blocked.
    pub(crate) fn ensure_address_completion(
        &self,
        account: i64,
        prefix: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<Correspondent>, String> {
        if prefix.trim().is_empty() {
            return Ok(Vec::new());
        }
        match self.begin_address_completion(account, false)? {
            AddressSlot::Ready => self.address_completion_rank(account, prefix, now_ms, limit),
            AddressSlot::Waiting(build) => {
                Self::wait_address_build(&build);
                self.address_completion_rank(account, prefix, now_ms, limit)
            }
            AddressSlot::Started(build, since) => {
                let stored = self
                    .load_address_completion(account, since)
                    .and_then(|loaded| {
                        self.store_loaded_address_completion(account, &build, since, loaded)
                    });
                self.finish_address_build(account, &build);
                stored?;
                self.address_completion_rank(account, prefix, now_ms, limit)
            }
        }
    }

    /// Called when compose opens and after a sync. No account returns without
    /// reading: there is nothing to warm, and that is not a failed build.
    /// Unbuilt reads every row. Already stored adds only past the watermark,
    /// and a build already running is left to finish rather than scanned again.
    pub(crate) fn warm_address_completion(&self) -> Result<(), String> {
        let account = {
            let store = self.store_read()?;
            match store.active_account().map_err(|e| e.to_string())? {
                Some(account) => account,
                None => return Ok(()),
            }
        };
        match self.begin_address_completion(account, true)? {
            AddressSlot::Ready | AddressSlot::Waiting(_) => Ok(()),
            AddressSlot::Started(build, since) => {
                let stored = self
                    .load_address_completion(account, since)
                    .and_then(|loaded| {
                        self.store_loaded_address_completion(account, &build, since, loaded)
                    });
                self.finish_address_build(account, &build);
                stored
            }
        }
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
        reads: [
            Mutex::new(Store::open_in_memory().expect("reader")),
            Mutex::new(Store::open_in_memory().expect("reader")),
        ],
        read_counts: Mutex::new(Store::open_in_memory().expect("reader")),
        read_open: Mutex::new(Store::open_in_memory().expect("reader")),
        read_index: Mutex::new(Store::open_in_memory().expect("reader")),
        readers_live: AtomicBool::new(false),
        blobs: BlobStore::open(&dir.join("blobs")).expect("blobs"),
        seeding: Mutex::new(HashMap::new()),
        seeding_marks: std::sync::atomic::AtomicU64::new(0),
        signin: Mutex::new(HashMap::new()),
        signin_changed: tokio::sync::watch::channel(0).0,
        held: Mutex::new(HashMap::new()),
        turns: Mutex::new(HashMap::new()),
        demo: AtomicBool::new(false),
        seeded: AtomicUsize::new(0),
        status_count: AtomicUsize::new(0),
        source: Mutex::new(String::new()),
        sync_error: Mutex::new(None),
        stops: Mutex::new(std::collections::HashMap::new()),
        picked: Mutex::new(std::collections::HashSet::new()),
        caps: Mutex::new(std::collections::HashMap::new()),
        surveyed: Mutex::new(std::collections::HashSet::new()),
        outbox: Mutex::new(Vec::new()),
        draining: AtomicBool::new(false),
        reindexing: AtomicBool::new(false),
        draft_dirty: Mutex::new(std::collections::HashSet::new()),
        folder_sync_inflight: Mutex::new(HashSet::new()),
        folder_synced_at: Mutex::new(HashMap::new()),
        pending_notify: Mutex::new(Vec::new()),
        pending_elsewhere: Mutex::new(Vec::new()),
        pending_alerts: Mutex::new(Vec::new()),
        last_sync_ms: std::sync::atomic::AtomicI64::new(0),
        extraction_gen: std::sync::atomic::AtomicI64::new(0),
        mail_gen: std::sync::atomic::AtomicI64::new(0),
        open_view: tokio::sync::watch::channel(None).0,
        folder_seen: Mutex::new(HashMap::new()),
        ui_touch_ms: std::sync::atomic::AtomicI64::new(0),
        server_total: std::sync::atomic::AtomicUsize::new(0),
        shown_once: Mutex::new(std::collections::HashSet::new()),
        tokens: Arc::new(ViewTokens::new()),
        account_id: account,
        data_dir: dir.display().to_string(),
        address_completion: Mutex::new(AddressCompletionCache::default()),
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
        assert!(
            ok("/data/Petrel/staged/1-0/dropped.txt"),
            "staged in a directory of its own"
        );
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
                ..Default::default()
            },
        );
        assert!(all.get(&1).copied().unwrap_or_default().is_gmail);
        assert!(!all.get(&2).copied().unwrap_or_default().is_gmail);
    }

    /// Surveyed means a survey's findings are in the store, per account.
    /// The server's capabilities do not stand in for it: the launch probe
    /// records those before it has stored anything.
    #[test]
    fn an_account_counts_as_surveyed_only_once_its_survey_is_stored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = test_state(dir.path());
        assert!(!state.surveyed(1));
        state.set_caps(
            1,
            ServerCaps {
                has_move: true,
                has_uidplus: true,
                is_gmail: false,
                ..Default::default()
            },
        );
        assert!(!state.surveyed(1), "capabilities are not a stored survey");
        state.mark_surveyed(1);
        assert!(state.surveyed(1));
        assert!(!state.surveyed(2), "one account's survey is not another's");
    }
}
