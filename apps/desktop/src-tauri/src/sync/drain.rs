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
            // The overlap guard is one flag across every account, and losing
            // to it must not lose the wake-up: a notification arriving while
            // another account drains used to be consumed and dropped, leaving
            // this account's queue waiting for the next unrelated signal.
            // The loser retries until the guard is free.
            while !drain_actions(
                Arc::clone(&state),
                account,
                cfg.clone(),
                has_move,
                has_uidplus,
                state.server_is_gmail.load(Ordering::Relaxed),
            )
            .await
            {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
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
    // Whether this call held the floor: false only when another drain was
    // already running, so the caller knows to come back rather than treat
    // the queue as attended to.
) -> bool {
    use petrel_engine::actions::ActionKind;

    // Refuse to overlap. compare_exchange rather than a load-then-store: two
    // tasks arriving together would both see `false` and both proceed.
    if state
        .draining
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    let _guard = DrainGuard(Arc::clone(&state));

    let pending = match state.store.lock().map(|s| s.pending_actions(account)) {
        Ok(Ok(p)) => p,
        _ => return true,
    };
    if pending.is_empty() {
        return true;
    }
    log_sync(&format!("draining {} queued change(s)", pending.len()));

    let mut delivered = 0usize;
    let mut stuck = 0usize;
    let mut undeliverable = 0usize;
    for item in pending {
        let Ok(kind) = serde_json::from_str::<ActionKind>(&item.kind_json) else {
            continue;
        };
        // No UID survived locally — a move destroyed the placement that held
        // it, or a UIDVALIDITY reset declared the number a lie. Before giving
        // up, ask the server the question recovery asks, scoped to this one
        // message: which of your numbers carries this Message-ID? The
        // candidates are the folders the store last saw the message in, and
        // a hit heals the placement that lost its number.
        let mut resolved = item.uid.map(|u| (u, item.folder_path.clone()));
        let mut search_failed = false;
        if resolved.is_none()
            && let Some(msgid) = item.msgid.as_deref().filter(|m| !m.is_empty())
        {
            for path in &item.candidate_paths {
                match petrel_providers::imap::uids_for_message_id(&cfg, path, msgid).await {
                    Ok(uids) => {
                        if let Some(u) = uids.last().copied() {
                            log_sync(&format!(
                                "action {}: {path} answers to the Message-ID with UID {u}",
                                item.action_id
                            ));
                            if let Ok(store) = state.store.lock() {
                                let _ = store.heal_placement_uid(item.message_id, account, path, u);
                            }
                            resolved = Some((u, path.clone()));
                            break;
                        }
                    }
                    // A failed search says nothing about the message — the
                    // network did not answer, so the action stays queued and
                    // the next drain asks again.
                    Err(e) => {
                        search_failed = true;
                        log_sync(&format!(
                            "action {}: search of {path} failed: {e}",
                            item.action_id
                        ));
                    }
                }
            }
        }
        let Some((uid, folder_path)) = resolved else {
            if search_failed {
                stuck += 1;
                continue;
            }
            // Every folder we know of answered, and none holds it — or it has
            // no Message-ID to ask about. There is no server copy for this
            // action to change, and retrying cannot learn more. Out of the
            // queue it goes, by name, in the log. The state is per action and
            // an action can carry several messages: the last writer wins, but
            // every terminal path leaves 'queued', which is what matters.
            undeliverable += 1;
            if let Ok(store) = state.store.lock() {
                let _ = store.mark_action_state(item.action_id, "undeliverable");
            }
            log_sync(&format!(
                "action {}: no server copy answers to it; marked undeliverable",
                item.action_id
            ));
            continue;
        };
        let folder = if folder_path.is_empty() {
            "INBOX".to_string()
        } else {
            folder_path
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
                        let moved = match petrel_providers::imap::move_uid(
                            &cfg, &folder, uid, &to, has_move,
                        )
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
                        };
                        // The server confirmed the move: the source placement
                        // goes now, from here, not from whatever sync pass
                        // next looks. A fetch that raced the delivery may
                        // have re-added it, and a placement the server no
                        // longer backs is how a conversation ends up haunting
                        // both its folder and the inbox.
                        if moved.is_ok()
                            && let Ok(store) = state.store.lock()
                        {
                            let _ = store.remove_placement(item.message_id, account, &folder);
                        }
                        moved
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
    let tail = if undeliverable > 0 {
        format!(", {undeliverable} undeliverable")
    } else {
        String::new()
    };
    log_sync(&format!(
        "drained {delivered} change(s), {stuck} still queued{tail}"
    ));
    true
}
