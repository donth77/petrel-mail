//! Keeping the store and the server in step: the sync cycle, the folders it covers, and the workers that run it.

pub(crate) mod backfill;
pub(crate) mod drafts;
pub(crate) mod drain;

use crate::diag::{friendly_sync_error, log_sync};
use crate::send::{send_due, spawn_outbox_clock};
use crate::state::{AppState, now_ms};
use crate::sync::backfill::spawn_backfill;
use crate::sync::drain::{drain_actions, spawn_drain_worker};
use petrel_engine::store::Store;
use petrel_providers::imap::ImapConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) fn spawn_real_sync(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
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
        // If another drain holds the floor the fetch proceeds without it —
        // the store's pending checks protect what is queued, and the drain
        // worker retries until the floor frees.
        let _ = drain_actions(
            Arc::clone(&state),
            account,
            cfg.clone(),
            has_move,
            has_uidplus,
            account_is_gmail(&cfg),
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
            run_thrid_sweep(&state, account, &cfg).await;
        }

        state.seeding.store(false, Ordering::Relaxed);

        // The first pass may have re-listed messages whose move the drain has
        // since delivered; sweep once now rather than waiting out the first
        // IDLE, so a conversation never spends the first half hour standing
        // in both its folder and the inbox.
        reconcile_ghost_placements(&state, account, &cfg).await;
        // The bin's clock starts at launch too, not only on the next poll:
        // with IDLE holding a quiet account open, "the next cycle" can be
        // hours away, and mail would sit in the bin unstamped until then.
        tend_the_bin(&state, account).await;

        // History fills in behind the present, on its own clock — see
        // spawn_backfill for why it is not part of the poll loop.
        spawn_backfill(Arc::clone(&state), account, cfg.clone());

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
            let _ = drain_actions(
                Arc::clone(&state),
                account,
                cfg.clone(),
                has_move,
                has_uidplus,
                account_is_gmail(&cfg),
            )
            .await;
            send_due(Arc::clone(&state), account).await;
            reconcile_ghost_placements(&state, account, &cfg).await;
            tend_the_bin(&state, account).await;

            // One connection for the whole account, STATUS-gated per folder:
            // a quiet cycle costs a line per folder, not a login per folder.
            let (fresh, failures) = run_sync_cycle(&state, account, &cfg, false).await;
            if account_is_gmail(&cfg) {
                // One round trip when nothing changed; live labels when it did.
                run_label_sweep(&state, account, &cfg).await;
                run_thrid_sweep(&state, account, &cfg).await;
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
                // Floored by the last-seen UIDNEXT: moving the newest message
                // out of a folder drops max_uid, and a watermark that falls
                // re-fetches mail the server still holds there — the moved
                // conversation walking straight back into the inbox.
                since_uid: {
                    let held = store.max_uid(*fid).ok().flatten().unwrap_or(0);
                    let next = store.folder_uidnext(*fid).ok().flatten().unwrap_or(0);
                    held.max(next.saturating_sub(1))
                },
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
        // Gmail's custom flags are labels, and the label sweep owns them.
        let want_keywords = !account_is_gmail(cfg);
        petrel_providers::imap::sync_pass(cfg, &passes, want_keywords, |index, uid, flags, raw| {
            // Compression happens out here, before the lock: one 20MB
            // attachment message compressed inside it stalled every click
            // and count in the app for the duration (measured at 11s).
            let _ = st.blobs.write(raw);
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
                keyword_updates,
                total,
            } => {
                server_total += *total as usize;
                let mut reflagged = 0usize;
                let mut retagged = 0usize;
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
                    // Keywords other clients set become tags here, and ones
                    // they removed stop being tags — the inbound half of the
                    // tag story on servers where a tag is an IMAP keyword.
                    if !keyword_updates.is_empty() {
                        retagged = store
                            .apply_keywords(account, *folder_id, keyword_updates)
                            .unwrap_or(0);
                    }
                }
                if verbose || *fetched > 0 || reflagged > 0 || retagged > 0 {
                    let tags = if retagged > 0 {
                        format!(", {retagged} tag change(s)")
                    } else {
                        String::new()
                    };
                    log_sync(&format!(
                        "{path}: {fetched} fetched, {reflagged} flag update(s){tags}"
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
    if failures == 0 {
        state.last_sync_ms.store(now_ms(), Ordering::Relaxed);
    }
    if !arrivals.is_empty() {
        apply_rules_to(state, account, &arrivals);
    }
    (fresh, failures)
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
        let mut store = state.store()?;
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
                let _ = st.blobs.write(raw);
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
        let mut store = state.store()?;
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
pub(crate) fn folders_to_sync_from(store: &Store, account: i64) -> Vec<(String, String, i64)> {
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

/// Ingests one fetched message, absorbing a parser panic instead of letting
/// it poison the store lock.
///
/// The sanitizer's rule is "salvage, never judge", but a bug in salvage is a
/// panic — and this callback holds the store lock, so before this fence one
/// hostile message did not cost one message, it cost every pane of the app
/// until relaunch (found the hard way: an HTML-only newsletter with an emoji
/// and a byte-walking tag stripper). The panic is still a bug and still gets
/// fixed; it is just no longer an outage while it waits to be found.
pub(crate) fn ingest_fenced(
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

/// Keeps the bin's clock running, and empties what has run out.
///
/// Off unless `trashRetentionDays` says otherwise, because deleting
/// somebody's mail on a timer is a promise to opt into rather than a
/// default to discover. The clock itself is maintained either way: turning
/// expiry on a month from now should not then treat everything in the bin
/// as a month old, nor as brand new.
async fn tend_the_bin(state: &Arc<AppState>, account: i64) {
    let now = crate::state::now_ms();
    let days: Option<i64> = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        let _ = store.refresh_trash_clock(account, now);
        store
            .settings()
            .ok()
            .and_then(|s| {
                s.get("trashRetentionDays")
                    .and_then(|v| v.parse::<i64>().ok())
            })
            .filter(|d| *d > 0)
    };
    let Some(days) = days else { return };
    let expired = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        store.trash_expired(account, days, now).unwrap_or_default()
    };
    if expired.is_empty() {
        return;
    }
    log_sync(&format!(
        "trash expiry: {} message(s) older than {days} day(s)",
        expired.len()
    ));
    let _ = crate::commands::triage::destroy_trashed(state, account, expired).await;
}

/// Whether this account's server is Gmail — asked of the account's own
/// configuration rather than of shared state.
///
/// `AppState::server_is_gmail` is one flag for the whole app, written by
/// whichever account probed last: with a Gmail account beside a Dovecot one
/// it says "Gmail" for both half the time. Everything below that must be
/// right *per account* — whether a tag is a label or a keyword, whether the
/// label sweeps are worth running — asks this instead. The shared flag
/// stays for the UI's benefit and is no longer consulted for correctness.
fn account_is_gmail(cfg: &ImapConfig) -> bool {
    let host = cfg.host.to_ascii_lowercase();
    host.contains("gmail") || host.contains("googlemail") || host.ends_with("google.com")
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
            if a.notify {
                // Said through the same announcer ordinary arrivals use:
                // the next status poll carries it out, and the UI applies
                // its own pause and level rules before saying anything.
                let who = parsed
                    .from_display
                    .clone()
                    .filter(|d| !d.is_empty())
                    .or_else(|| parsed.from_addr.clone())
                    .unwrap_or_default();
                let subject = parsed.subject.clone().unwrap_or_default();
                if let Ok(mut pending) = state.pending_notify.lock() {
                    pending.push((who, subject));
                }
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

/// Gmail's own conversation ids, swept the way labels are.
///
/// JWZ threading works from References headers, and mail that arrives
/// without them threads alone — a Gmail inbox counted ~655 conversations
/// where Gmail's UI said ~271. X-GM-THRID is Gmail's answer; where known it
/// is authoritative, and each sweep regroups whatever it learned.
async fn run_thrid_sweep(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) {
    let since: Option<u64> = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.settings().ok())
        .and_then(|s| s.get("gmail_thrid_modseq").and_then(|v| v.parse().ok()));
    let bound: u32 = std::env::var("PETREL_LABEL_SWEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    match petrel_providers::imap::sweep_gmail_thrids(cfg, "[Gmail]/All Mail", bound, since).await {
        Ok(sweep) => {
            let (applied, regrouped) = {
                let Ok(store) = state.store.lock() else {
                    return;
                };
                let folder = store.folder_for_role(account, "archive").ok().flatten();
                let applied = folder
                    .and_then(|fid| store.apply_gm_thrids(fid, &sweep.thrids).ok())
                    .unwrap_or(0);
                let regrouped = if applied > 0 {
                    store.regroup_gmail_threads(account).unwrap_or(0)
                } else {
                    0
                };
                (applied, regrouped)
            };
            if applied > 0 {
                log_sync(&format!(
                    "threads: {} reported, {applied} learned, {regrouped} rethreaded",
                    sweep.thrids.len()
                ));
            }
            if let (Some(m), Ok(store)) = (sweep.modseq, state.store.lock()) {
                let _ = store.set_setting("gmail_thrid_modseq", &m.to_string());
            }
        }
        // Not fatal: threading falls back to what References could prove.
        Err(e) => log_sync(&format!("thread sweep failed: {e}")),
    }
}

/// Drops placements the server no longer backs.
///
/// The windowed sync only ever adds and updates: a message moved out of a
/// folder on the server — by our own drain, a rule, or another client — left
/// its old placement behind forever, so the conversation stood in both its
/// folder and the inbox. STATUS is the cheap tell: when a folder's server
/// count falls below the store's UID-bearing placement count, something we
/// hold is gone, and one SEARCH names the survivors. Server counts at or
/// above ours are the fetch's business, not this sweep's — new mail is not a
/// ghost. Equal counts can mask one ghost plus one unfetched arrival; the
/// next arrival or move tips the balance and the sweep catches it then.
async fn reconcile_ghost_placements(
    state: &Arc<AppState>,
    account: i64,
    cfg: &petrel_providers::imap::ImapConfig,
) {
    let candidates: Vec<(i64, String, i64)> = {
        let Ok(store) = state.store.lock() else {
            return;
        };
        let Ok(folders) = store.folders(account) else {
            return;
        };
        folders
            .into_iter()
            .filter_map(|f| {
                let n = store.uid_placement_count(f.id).ok()?;
                (n > 0).then_some((f.id, f.path, n))
            })
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let paths: Vec<String> = candidates.iter().map(|c| c.1.clone()).collect();
    let Ok(counts) = petrel_providers::imap::folder_counts(cfg, &paths).await else {
        return;
    };
    for (folder_id, path, local) in candidates {
        let Some((_, server)) = counts.iter().find(|(p, _)| *p == path) else {
            continue;
        };
        if i64::from(*server) == local {
            continue;
        }
        let present: std::collections::HashSet<u32> =
            match petrel_providers::imap::uids_in_folder(cfg, &path).await {
                Ok(uids) => uids.into_iter().collect(),
                Err(e) => {
                    log_sync(&format!("{path}: reconcile sweep failed: {e}"));
                    continue;
                }
            };
        // Outward: placements the server no longer backs go.
        let removed = state
            .store
            .lock()
            .ok()
            .and_then(|s| s.remove_placements_absent(folder_id, &present).ok())
            .unwrap_or(0);
        if removed > 0 {
            log_sync(&format!(
                "{path}: {removed} placement(s) the server no longer holds removed"
            ));
        }
        // Inward: server UIDs the store never placed. The windowed sync can
        // close a watermark over a gap — a draft revision saved by webmail
        // landed between a backfill's endpoint and the forward window and
        // was skipped forever, watermark shut behind it. Only once the
        // backfill has finished its walk: before that, "missing" is most of
        // the folder and belongs to the backfill, not to this sweep.
        let (stored, backfilled) = {
            let Ok(store) = state.store.lock() else {
                continue;
            };
            let stored: std::collections::HashSet<u32> = store
                .placement_uids(folder_id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            (
                stored,
                store.backfill_floor(folder_id).ok().flatten() == Some(1),
            )
        };
        if !backfilled {
            continue;
        }
        let mut missing: Vec<u32> = present.difference(&stored).copied().collect();
        missing.sort_unstable();
        if missing.is_empty() {
            continue;
        }
        // Bounded: a legitimate gap is a handful; thousands means something
        // larger is wrong and one cycle should not fetch a mailbox.
        let overflow = missing.len().saturating_sub(200);
        missing.truncate(200);
        let fetched = petrel_providers::imap::fetch_uids_each(
            cfg,
            &path,
            &missing,
            |uid, flags, raw| {
                let Ok(mut store) = state.store.lock() else { return };
                // Two copies of one Message-ID, both live on the server right
                // now — this sweep only fetches UIDs the server just named.
                // Dedupe would fold this one into the copy already held and
                // throw its content away; a draft edited apart on the server,
                // or a double-delivered message, is two rows there and stays
                // two rows here.
                if let Some(parsed) = petrel_mime::parse_message(raw)
                    && let Some(mid) = parsed.message_id.as_deref()
                    && let Ok(Some(existing)) = store.message_by_msgid(account, mid)
                    && matches!(store.placement_uid(existing, folder_id), Ok(Some(Some(held))) if held != i64::from(uid))
                {
                    match store.ingest_raw_second_copy(&state.blobs, account, Some(folder_id), uid, raw) {
                        Ok(ingested) => {
                            let _ = store.set_message_flags(ingested.message_id, flags);
                            log_sync(&format!(
                                "{path}: uid {uid} is a second live copy of a stored message; kept as its own"
                            ));
                        }
                        Err(e) => log_sync(&format!("{path}: second copy uid {uid} failed: {e}")),
                    }
                    return;
                }
                let _ = ingest_fenced(&mut store, &state.blobs, account, folder_id, uid, flags, raw);
            },
        )
        .await
        .unwrap_or(0);
        if fetched > 0 || overflow > 0 {
            log_sync(&format!(
                "{path}: {fetched} message(s) the store was missing fetched{}",
                if overflow > 0 {
                    format!(" ({overflow} more next cycle)")
                } else {
                    String::new()
                }
            ));
        }
    }
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
