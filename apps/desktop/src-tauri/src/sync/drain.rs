//! Local changes reaching the server: the drain worker and the pass it runs.

use crate::diag::log_sync;
use crate::send::send_due;
use crate::state::AppState;
use petrel_providers::imap::ImapConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Delivers queued triage as soon as there is any, rather than when the next
/// sync happens to run.
///
/// Debounced, because triage comes in bursts: working down an inbox is a run of
/// archives a few hundred milliseconds apart, and one connection carrying all
/// of them beats one connection each. A second of latency is invisible to the
/// person doing it and saves a login per keystroke.
pub(crate) fn spawn_drain_worker(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
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
pub(crate) async fn drain_actions(
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

    let pending = match state.store.lock().map(|s| s.pending_actions(account)) {
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
