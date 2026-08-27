//! Acting on mail: triage and its undo, tags, and folders.

use crate::config::imap_config_for;
use crate::diag::log_sync;
use crate::state::{AppState, active_account, note_ui_touch};
use petrel_engine::actions::{ActionKind, ActionReceipt};
use petrel_engine::store::FolderSummary;
use std::sync::Arc;
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
#[tauri::command]
pub fn list_folders(state: State<Arc<AppState>>) -> Result<Vec<FolderSummary>, String> {
    note_ui_touch(&state);
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
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
    store.remove_folder(folder_id).map_err(|e| e.to_string())
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
    let (cfg, items, uidplus) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let items = store.trash_contents(account).map_err(|e| e.to_string())?;
        (
            imap_config_for(&store, account),
            items,
            state
                .server_has_uidplus
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    };
    if items.is_empty() {
        return Ok("0/0".into());
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
    log_sync(&format!("trash emptied: {gone} removed, {kept} kept"));
    Ok(format!("{gone}/{kept}"))
}
