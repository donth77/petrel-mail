//! Local changes reaching the server: the drain worker and the pass it runs.

use crate::diag::log_sync;
use crate::state::AppState;
use petrel_providers::imap::ImapConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// How many times a permanently-refused action is asked again before it is
/// put out of the queue.
///
/// Not once: a single odd answer should not discard work somebody did, and a
/// folder deleted by accident can come back. Not indefinitely: that is the
/// behaviour this replaces, where one action failed on every cycle for days
/// and the change it carried was never made and never abandoned either.
const GIVE_UP_AFTER: i64 = 5;

/// Same kind, same folder, already-resolved UIDs: one STORE instead of one
/// connection per message.
fn flag_op(kind: &petrel_engine::actions::ActionKind) -> Option<(&'static str, bool)> {
    use petrel_engine::actions::ActionKind;
    match kind {
        ActionKind::MarkRead => Some(("\\Seen", true)),
        ActionKind::MarkUnread => Some(("\\Seen", false)),
        ActionKind::Star => Some(("\\Flagged", true)),
        ActionKind::Unstar => Some(("\\Flagged", false)),
        _ => None,
    }
}

/// How many following rows share this flag STORE.
///
/// Stops at the first row that would need a search or a different folder —
/// those must stay in order relative to this action.
fn consecutive_flag_uids(
    pending: &[petrel_engine::store::PendingAction],
    start: usize,
    kind_json: &str,
    folder: &str,
    first_uid: u32,
    max: usize,
) -> Vec<(usize, i64, u32)> {
    let mut out = vec![(start, pending[start].action_id, first_uid)];
    let mut i = start + 1;
    while out.len() < max && i < pending.len() {
        let row = &pending[i];
        if row.kind_json != kind_json {
            break;
        }
        let later = if row.folder_path.is_empty() {
            "INBOX"
        } else {
            row.folder_path.as_str()
        };
        if later != folder {
            break;
        }
        let Some(uid) = row.uid else { break };
        out.push((i, row.action_id, uid));
        i += 1;
    }
    out
}

/// Whether the server's refusal is one that asking again cannot fix.
///
/// A mailbox that does not exist will not start existing on the two hundredth
/// try — that is a folder renamed or deleted out from under a queued change.
/// A broken pipe is the opposite kind of failure and must not count.
///
/// Anything unrecognised is treated as temporary, so a wrong guess costs a
/// retry rather than somebody's change.
fn permanent_refusal(error: &str) -> bool {
    let low = error.to_lowercase();
    low.contains("nonexistent")
        || low.contains("trycreate")
        || low.contains("doesn't exist")
        || low.contains("does not exist")
        || low.contains("no such mailbox")
        || low.contains("unknown mailbox")
}

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
            // Triage is done. If anything became due while we were in IMAP,
            // the send worker takes it — we must not await send_due here or
            // the next Send now waits on the next backlog the same way.
            state.nudge_send(account);
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
    // An action can carry several messages and arrives here once per message,
    // so a failing thread of ten would otherwise spend ten tries in a single
    // cycle. One count per action per pass.
    let mut counted: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut batched: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (idx, item) in pending.iter().enumerate() {
        if batched.contains(&idx) {
            continue;
        }
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
            ActionKind::MarkRead
            | ActionKind::MarkUnread
            | ActionKind::Star
            | ActionKind::Unstar => {
                let (flag, add) = flag_op(&kind).expect("flag kind");
                let group = consecutive_flag_uids(
                    &pending,
                    idx,
                    &item.kind_json,
                    &folder,
                    uid,
                    petrel_providers::imap::STORE_FLAG_BATCH,
                );
                let uids: Vec<u32> = group.iter().map(|(_, _, u)| *u).collect();
                let result =
                    petrel_providers::imap::store_flags(&cfg, &folder, &uids, flag, add).await;
                if result.is_ok() {
                    for (i, action_id, _) in &group {
                        if *i != idx {
                            batched.insert(*i);
                            delivered += 1;
                            if let Ok(store) = state.store.lock() {
                                let _ = store.mark_action_state(*action_id, "sent");
                            }
                        }
                    }
                }
                result
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
                let adding = matches!(kind, ActionKind::Tag);
                if looks_like_gmail {
                    petrel_providers::imap::store_gmail_labels(&cfg, &folder, uid, &name, adding)
                        .await
                } else {
                    // Everywhere else a tag travels as an IMAP keyword, which
                    // Dovecot persists beside the system flags. These actions
                    // used to sit 'queued' forever with nowhere to go.
                    let keyword = petrel_engine::keywords::tag_keyword(&name);
                    petrel_providers::imap::store_flag(&cfg, &folder, uid, &keyword, adding).await
                }
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
                let text = e.to_string();
                // A refusal retrying cannot fix is counted, and after enough
                // of them the action leaves the queue. Anything else — a
                // broken pipe, a sleeping laptop — is not counted at all:
                // the network comes back, and discarding somebody's change
                // because it went away would be the worse bug.
                let spent = permanent_refusal(&text) && counted.insert(item.action_id) && {
                    match state.store.lock() {
                        Ok(store) => {
                            let n = store.record_attempt(item.action_id).unwrap_or(0);
                            if n >= GIVE_UP_AFTER {
                                let _ = store.mark_action_state(item.action_id, "undeliverable");
                                true
                            } else {
                                false
                            }
                        }
                        Err(_) => false,
                    }
                };
                if spent {
                    undeliverable += 1;
                    log_sync(&format!(
                        "action {}: {text}; asked {GIVE_UP_AFTER} times, marked undeliverable",
                        item.action_id
                    ));
                } else {
                    stuck += 1;
                    log_sync(&format!(
                        "action {} could not be delivered: {text}",
                        item.action_id
                    ));
                }
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

#[cfg(test)]
mod refusal_tests {
    use super::permanent_refusal;

    #[test]
    fn a_missing_mailbox_is_permanent() {
        // The exact words a real account produced, 112 times.
        assert!(permanent_refusal(
            "imap: no response: code: None, info: Some(\"Mailbox doesn't exist: glassdoor+3022026 (0.002 secs).\")"
        ));
        assert!(permanent_refusal("[NONEXISTENT] Mailbox does not exist"));
        assert!(permanent_refusal("[TRYCREATE] No such mailbox"));
    }

    #[test]
    fn a_network_failure_is_not() {
        // Counting these would discard a change because a laptop slept.
        assert!(!permanent_refusal("imap: io: Broken pipe (os error 32)"));
        assert!(!permanent_refusal("connection reset by peer"));
        assert!(!permanent_refusal("operation timed out"));
    }

    #[test]
    fn anything_unrecognised_is_retried_rather_than_discarded() {
        assert!(!permanent_refusal("server said something new and strange"));
    }
}

#[cfg(test)]
mod batch_tests {
    use petrel_engine::store::PendingAction;

    fn row(kind: &str, folder: &str, uid: Option<u32>, action: i64) -> PendingAction {
        PendingAction {
            action_id: action,
            kind_json: kind.to_string(),
            payload_json: "{}".into(),
            message_id: action,
            uid,
            folder_path: folder.into(),
            msgid: None,
            candidate_paths: Vec::new(),
        }
    }

    #[test]
    fn consecutive_flag_uids_stop_at_a_gap_and_cap_at_a_hundred() {
        let mut pending: Vec<PendingAction> = (0..120)
            .map(|i| row("\"mark_read\"", "INBOX", Some(i as u32 + 1), i))
            .collect();
        pending[40].uid = None;
        let group = super::consecutive_flag_uids(
            &pending,
            0,
            "\"mark_read\"",
            "INBOX",
            1,
            petrel_providers::imap::STORE_FLAG_BATCH,
        );
        assert_eq!(group.len(), 40, "stops before the row with no UID");
        assert_eq!(group.last().map(|(_, _, u)| *u), Some(40));

        let full: Vec<PendingAction> = (0..150)
            .map(|i| row("\"mark_read\"", "INBOX", Some(i as u32 + 1), i))
            .collect();
        let group = super::consecutive_flag_uids(
            &full,
            0,
            "\"mark_read\"",
            "INBOX",
            1,
            petrel_providers::imap::STORE_FLAG_BATCH,
        );
        assert_eq!(group.len(), 100);
    }
}
