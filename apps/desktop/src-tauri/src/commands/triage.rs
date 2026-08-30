//! Acting on mail: triage and its undo, tags, and folders.

use crate::config::imap_config_for;
use crate::diag::log_sync;
use crate::state::{AppState, active_account, note_ui_touch};
use petrel_engine::actions::{ActionKind, ActionReceipt};
use petrel_engine::store::FolderSummary;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::State;

/// Applies a triage action locally and queues it. Returns the receipt the UI
/// needs to offer undo, so the frontend holds no state of its own about what it
/// just did.
#[tauri::command]
pub fn triage(
    thread_id: i64,
    kind: ActionKind,
    target: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<ActionReceipt, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
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

#[tauri::command]
pub fn undo_triage(action_id: i64, state: State<Arc<AppState>>) -> Result<bool, String> {
    let store = state.store()?;
    let undone = store.undo_action(action_id).map_err(|e| e.to_string())?;
    // An undo can leave other queued work behind it, and the row it cancelled
    // is gone from the queue — either way the server's picture just changed.
    state.drain_signal.notify_one();
    Ok(undone)
}

/// Creates a tag, or returns the one already there — same shape as folders.
#[tauri::command]
pub fn create_tag(name: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store
        .ensure_tag(account, &name, None)
        .map_err(|e| e.to_string())
}

/// Corrects a tag's name. The colour and every tagged message come with it.
#[tauri::command]
pub fn rename_tag(tag_id: i64, name: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    store.rename_tag(tag_id, &name).map_err(|e| e.to_string())
}

/// Sets a tag's colour. Local by design: no provider has a field for it.
#[tauri::command]
pub fn set_tag_colour(
    tag_id: i64,
    colour: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    store
        .set_tag_colour(tag_id, &colour)
        .map_err(|e| e.to_string())
}

/// Removes a tag from the account and from every message carrying it.
#[tauri::command]
pub fn delete_tag(tag_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    store.delete_tag(tag_id).map_err(|e| e.to_string())
}

/// Folders for the move picker (V).
///
/// `account` names one explicitly. Absent it means the account on screen,
/// which is what every list in the window wants — but the export pane offers
/// a row per account, and a folder list borrowed from whichever one happens
/// to be active would name places that account's export cannot find.
#[tauri::command]
pub fn list_folders(
    account: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<Vec<FolderSummary>, String> {
    note_ui_touch(&state);
    let store = state.store()?;
    let account = match account {
        Some(id) => id,
        None => match store.active_account().map_err(|e| e.to_string())? {
            Some(id) => id,
            None => return Ok(Vec::new()),
        },
    };
    store.folders(account).map_err(|e| e.to_string())
}

/// Creates a folder the user named, or returns the one already there. The
/// picker offers this on the end of the same keystroke as choosing one.
#[tauri::command]
pub fn create_folder(path: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let (account, id, cfg) = {
        let store = state.store()?;
        let account = active_account(&store)?;
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
pub async fn rename_folder(
    folder_id: i64,
    new_path: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let (cfg, old_path) = {
        let store = state.store()?;
        let account = active_account(&store)?;
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
    let mut store = state.store()?;
    store
        .rename_folder(folder_id, &new_path)
        .map_err(|e| e.to_string())
}

/// Deletes a folder — on the server first. The server also deletes whatever
/// mail the folder still holds, which is why the UI confirms in those words;
/// the store keeps its message rows and blobs regardless, so nothing already
/// synced is destroyed.
#[tauri::command]
pub async fn delete_folder(folder_id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let (cfg, path) = {
        let store = state.store()?;
        let account = active_account(&store)?;
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
    let mut store = state.store()?;
    let took = store.remove_folder(folder_id).map_err(|e| e.to_string())?;
    if took > 0 {
        log_sync(&format!(
            "folder deleted: {took} message(s) that lived only there went with it"
        ));
    }
    Ok(())
}

/// Empties the bin: everything in Trash, and in any folder filed under it,
/// expunged on the server and tombstoned here.
///
/// The one action in the app with no undo, which is why it is a button
/// someone presses in the Trash itself and not a thing that happens on a
/// timer. Retention already reaps *tombstones* after their grace period;
/// this is the person saying "now", about mail they can see.
///
/// A message the server refuses to expunge stays — reported, not pretended
/// away. Emptying half a bin and saying it is empty would be the one
/// outcome worse than not emptying it.
#[tauri::command]
pub async fn empty_trash(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let (account, items) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let items = store.trash_contents(account).map_err(|e| e.to_string())?;
        (account, items)
    };
    let (gone, kept) = destroy_trashed(&state, account, items).await?;
    Ok(format!("{gone}/{kept}"))
}

/// Expunges a set of trashed messages and tombstones them here.
///
/// Shared by the button and the clock so they cannot drift: emptying the
/// bin by hand and emptying it by expiry are the same act on a different
/// selection, and two implementations of "destroy this mail" is one too
/// many. Returns (removed, kept).
pub(crate) async fn destroy_trashed(
    state: &Arc<AppState>,
    account: i64,
    items: Vec<(String, u32, i64)>,
) -> Result<(usize, usize), String> {
    let (cfg, uidplus) = {
        let store = state.store()?;
        (
            imap_config_for(&store, account),
            state
                .server_has_uidplus
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    };
    if items.is_empty() {
        return Ok((0, 0));
    }
    let mut gone = 0usize;
    let mut kept = 0usize;
    for (path, uid, message_id) in items {
        let removed = match &cfg {
            Some(cfg) => {
                match petrel_providers::imap::expunge_uid(cfg, &path, uid, uidplus).await {
                    Ok(_) => true,
                    Err(e) => {
                        log_sync(&format!("empty trash: {path} uid {uid}: {e}"));
                        false
                    }
                }
            }
            // No server for this account: local-only mail is ours to drop.
            None => true,
        };
        if removed {
            if let Ok(store) = state.store.lock() {
                let _ = store.tombstone_message(message_id);
            }
            gone += 1;
        } else {
            kept += 1;
        }
    }
    log_sync(&format!("trash: {gone} removed, {kept} kept"));
    Ok((gone, kept))
}

/// The order somebody dragged their folders into.
///
/// Local only, and it never touches the server: IMAP has no notion of an
/// order, so there is nothing to push and nothing that can come back to
/// contradict it. That also makes this the rare folder command that cannot
/// half-fail, which is why it has no rollback.
#[tauri::command]
pub fn reorder_folders(ids: Vec<i64>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.reorder_folders(&ids).map_err(|e| e.to_string())
}

/// The order somebody dragged their tags into. Local, for the same reason.
#[tauri::command]
pub fn reorder_tags(ids: Vec<i64>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.reorder_tags(&ids).map_err(|e| e.to_string())
}

/// How many messages a folder holds, so a confirmation can name the number.
#[tauri::command]
pub fn folder_message_count(folder_id: i64, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    store
        .folder_message_count(folder_id)
        .map_err(|e| e.to_string())
}

/// Marks everything in a folder read, or unread.
///
/// Done here rather than through the action queue, which is per message: a
/// folder with ten thousand messages in it would put ten thousand rows in that
/// queue and spend ten thousand round trips draining them. IMAP will set the
/// whole mailbox in one command, so that is what this sends, the same way
/// Empty Trash does its own work rather than queuing it.
///
/// Local first, then the server, which is the opposite of `rename_folder` and
/// deliberately so: this is not destructive, the local half is instant, and
/// somebody who marks a folder read wants the number to move now rather than
/// after a round trip. A server that refuses leaves the two disagreeing until
/// the next sync reconciles, which is the ordinary state of every flag here.
#[tauri::command]
pub async fn mark_folder_read(
    folder_id: i64,
    read: bool,
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let (cfg, paths, changed) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        // The subtree, because that is what "all" means on a row with folders
        // under it. IMAP has no recursive STORE, so this is one command per
        // mailbox — sixteen for a real Archive, against the ten thousand a
        // per-message queue would have sent.
        let paths: Vec<String> = store
            .folder_subtree(folder_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        let changed = store
            .mark_folder_seen(folder_id, read)
            .map_err(|e| e.to_string())?;
        (imap_config_for(&store, account), paths, changed)
    };
    if let Some(cfg) = cfg {
        let mut total = 0u32;
        for path in &paths {
            match petrel_providers::imap::store_flag_all(&cfg, path, "\\Seen", read).await {
                Ok(n) => total += n,
                // Reported, not swallowed. The local half already happened and
                // the next sync will notice the disagreement; what must not
                // happen is silence about a server that said no.
                Err(e) => return Err(e.to_string()),
            }
        }
        log_sync(&format!(
            "marked {} across {} mailbox(es), {total} message(s)",
            if read { "read" } else { "unread" },
            paths.len()
        ));
    }
    Ok(changed)
}

/// Moves everything in a folder to the Trash.
///
/// The folder stays; only its contents go. Recoverable exactly as any other
/// binning is — the mail is in the Trash until somebody empties it — which is
/// why this is a confirm rather than the undo the per-message actions get:
/// capturing prior state for ten thousand messages to make one undo entry is
/// a lot of database for a gesture whose inverse is "drag it back".
#[tauri::command]
pub async fn trash_folder_contents(
    folder_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let (cfg, from_paths, to_path, to_id, has_move) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let from_paths: Vec<String> = store
            .folder_subtree(folder_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        let to_id = store
            .folder_for_role(account, "trash")
            .map_err(|e| e.to_string())?
            .ok_or("this account has no Trash")?;
        let to_path = store
            .folder_path(to_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        (
            imap_config_for(&store, account),
            from_paths,
            to_path,
            to_id,
            state.server_has_move.load(Ordering::Relaxed),
        )
    };
    if from_paths.contains(&to_path) {
        return Err("that is the Trash".into());
    }
    // Server first here, unlike marking read: this one moves mail, and a local
    // move that the server refused would show an empty folder that is still
    // full on every other client.
    if let Some(cfg) = cfg {
        let mut moved = 0u32;
        for from in &from_paths {
            moved += petrel_providers::imap::move_all(&cfg, from, &to_path, has_move)
                .await
                .map_err(|e| e.to_string())?;
        }
        log_sync(&format!(
            "moved {moved} message(s) from {} mailbox(es) to {to_path}",
            from_paths.len()
        ));
    }
    let mut store = state.store()?;
    store
        .move_folder_contents(folder_id, to_id)
        .map_err(|e| e.to_string())
}
