//! History filling in behind the present, in strides that yield to whoever is using the app.

use crate::diag::log_sync;
use crate::state::{AppState, ui_recently_active};
use crate::sync::{folders_to_sync_from, ingest_fenced};
use petrel_providers::imap::ImapConfig;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// History's own clock. Backfill used to tick only after a poll cycle,
/// which with IDLE meant "when new mail happens to arrive, or every twenty
/// minutes" — a quiet mailbox's history arrived at a crawl. This task walks
/// strides on its own connection at its own pace: briskly while there is
/// work, dormant once every folder's floor reaches 1. Strides stay small,
/// so a click or a poll never waits long behind one.
pub(crate) fn spawn_backfill(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
    tauri::async_runtime::spawn(async move {
        loop {
            yield_to_user(&state).await;
            // Recent history first; the deep archive once that is quiet.
            let worked = run_backfill_tick(&state, account, &cfg).await
                || run_allmail_tick(&state, account, &cfg).await;
            if worked {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            } else {
                // Done, or a folder list that may change later: look again
                // rarely rather than never.
                tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            }
        }
    });
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
            let _ = st.blobs.write(raw);
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

/// One stride of the Gmail All Mail walk — the account's full history,
/// claimed cheaply.
///
/// All Mail holds a copy of everything, so most of it is mail already here.
/// A stride lists (UID, Message-ID) pairs — a line per message — claims the
/// known ones by writing their All Mail placement (which is what the Archive
/// view reads), and downloads bodies only for strangers: the archived-and-
/// unlabeled mail no other folder will ever surface. The floor records how
/// deep the walk has asked, exactly like ordinary backfill; floor 1 is done.
async fn run_allmail_tick(state: &Arc<AppState>, account: i64, cfg: &ImapConfig) -> bool {
    let chunk: u32 = std::env::var("PETREL_ALLMAIL_CHUNK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    let target = {
        let Ok(store) = state.store.lock() else {
            return false;
        };
        // Only where folders are labels: elsewhere Archive is an ordinary
        // folder that ordinary backfill already walks.
        let is_gmail = store
            .accounts()
            .ok()
            .and_then(|accs| accs.into_iter().find(|a| a.id == account))
            .map(|a| a.kind == "gmail")
            .unwrap_or(false);
        if !is_gmail {
            return false;
        }
        let Some(folder_id) = store.folder_for_role(account, "archive").ok().flatten() else {
            return false;
        };
        let Some(path) = store.folder_path(folder_id).ok().flatten() else {
            return false;
        };
        match store.backfill_floor(folder_id).ok().flatten() {
            Some(1) => return false, // done
            floor => (folder_id, path, floor),
        }
    };
    let (folder_id, path, floor) = target;

    let ceiling = match floor {
        Some(f) => f,
        // A fresh walk starts at the top of the mailbox as it is today. New
        // arrivals above this land through inbox sync and the label sweep.
        None => match petrel_providers::imap::folder_uidnext(cfg, &path).await {
            Ok(Some(next)) => next,
            Ok(None) => return false,
            Err(e) => {
                log_sync(&format!("all-mail walk could not start: {e}"));
                return false;
            }
        },
    };
    if ceiling <= 1 {
        if let Ok(mut store) = state.store.lock() {
            let _ = store.set_backfill_floor(folder_id, 1);
        }
        return false;
    }
    let first = ceiling.saturating_sub(chunk).max(1);
    let last = ceiling - 1;

    let listed = match petrel_providers::imap::fetch_id_map_range(cfg, &path, first, last).await {
        Ok(l) => l,
        Err(e) => {
            log_sync(&format!("all-mail stride failed: {e}"));
            return true; // failed is not finished; retry next tick
        }
    };
    let mut claimed = 0usize;
    let mut strangers: Vec<u32> = Vec::new();
    {
        let Ok(store) = state.store.lock() else {
            return false;
        };
        for (uid, mid) in &listed {
            match mid
                .as_deref()
                .and_then(|m| store.message_by_msgid(account, m).ok().flatten())
            {
                Some(existing) => {
                    if store.place_message_at(existing, folder_id, *uid).is_ok() {
                        claimed += 1;
                    }
                }
                None => strangers.push(*uid),
            }
        }
    }
    let mut fetched = 0usize;
    if !strangers.is_empty() {
        let st = Arc::clone(state);
        fetched =
            petrel_providers::imap::fetch_uids_each(cfg, &path, &strangers, |uid, flags, raw| {
                let _ = st.blobs.write(raw);
                let Ok(mut store) = st.store.lock() else {
                    return;
                };
                if ingest_fenced(&mut store, &st.blobs, account, folder_id, uid, flags, raw)
                    == Some(true)
                {
                    st.seeded.fetch_add(1, Ordering::Relaxed);
                }
            })
            .await
            .unwrap_or(0);
    }
    if let Ok(mut store) = state.store.lock() {
        let _ = store.set_backfill_floor(folder_id, first);
    }
    if claimed > 0 || fetched > 0 {
        log_sync(&format!(
            "all-mail {path}: {claimed} claimed, {fetched} downloaded, down to uid {first}"
        ));
    }
    true
}

/// Parks a background task while the user is working. Returns when the UI
/// has been quiet for a beat — the spec's "interactive preempts backfill",
/// implemented as politeness rather than a queue.
async fn yield_to_user(state: &Arc<AppState>) {
    while ui_recently_active(state, 1500) {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
}
