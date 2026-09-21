//! Acting on mail: triage and its undo, tags, and folders.

use crate::config::{imap_config_for, imap_config_from_servers};
use crate::diag::log_sync;
use crate::state::{AppState, active_account, note_ui_touch};
use petrel_engine::actions::{ActionKind, ActionReceipt};
use petrel_engine::store::FolderSummary;
use std::sync::Arc;
use tauri::State;

/// Applies a triage action locally and queues it. Returns the receipt the UI
/// needs to offer undo, so the frontend holds no state of its own about what it
/// just did.
///
/// `message_id` names one message instead, for the view that lists per message.
/// Drafts is the only one, and there the conversation is the wrong unit: a
/// pushed draft shares its conversation's thread, so a verb aimed at the draft
/// row used to file the whole correspondence. Sent by the window rather than
/// inferred here, because which view the click came from is the window's to
/// know. Placement verbs only; the store refuses the rest.
#[tauri::command(async)]
pub fn triage(
    thread_id: i64,
    kind: ActionKind,
    target: Option<i64>,
    message_id: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<ActionReceipt, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    // The provider's placement model, not a per-call guess: on Gmail an
    // archive removes one label, on a classic server it replaces the folder.
    let policy = store.placement_policy(account).map_err(|e| e.to_string())?;
    let receipt = match message_id {
        Some(id) => store
            .apply_message_action(account, id, kind, target, policy)
            .map_err(|e| e.to_string())?,
        None => store
            .apply_thread_action(account, thread_id, kind, target, policy)
            .map_err(|e| e.to_string())?,
    };
    // Local change done; ask for it to be delivered. The lock is released as
    // this returns, so the drain is never waiting on the caller.
    state.nudge_drain(account);
    Ok(receipt)
}

#[tauri::command(async)]
pub fn undo_triage(action_id: i64, state: State<Arc<AppState>>) -> Result<bool, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    let undone = store.undo_action(action_id).map_err(|e| e.to_string())?;
    // An undo can leave other queued work behind it, and the row it cancelled
    // is gone from the queue — either way the server's picture just changed.
    state.nudge_drain(account);
    Ok(undone)
}

/// Creates a tag, or returns the one already there — same shape as folders.
#[tauri::command(async)]
pub fn create_tag(name: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store
        .ensure_tag(account, &name, None)
        .map_err(|e| e.to_string())
}

/// Corrects a tag's name. The colour and every tagged message come with it.
#[tauri::command(async)]
pub fn rename_tag(tag_id: i64, name: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    store.rename_tag(tag_id, &name).map_err(|e| e.to_string())
}

/// Sets a tag's colour. Local by design: no provider has a field for it.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
///
/// Here only. The server's copy is `push_folder`, which the caller awaits or
/// not as suits it — the picker has mail to file and the id is all it needs,
/// while the rail's New folder has nothing to do but say whether it worked.
/// This used to fire the server's create off on its own and forget it: a
/// failure reached the log and nowhere else, and the next sync, not finding
/// the folder on the server, deleted it here as well.
#[tauri::command(async)]
pub fn create_folder(path: String, state: State<Arc<AppState>>) -> Result<i64, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store
        .ensure_named_folder(account, &path)
        .map_err(|e| e.to_string())
}

/// Puts a folder made here on the server, and subscribes to it so webmail
/// shows it too. Ok means the server has it.
///
/// A folder the server already has, or a local one, is a no-op; so is one
/// belonging to an account other than the one on screen, which the sync of
/// its own account will create instead. A failure is returned rather than
/// only logged, so the person hears about it — and the folder stays waiting,
/// which is what makes the next sync try again.
#[tauri::command]
pub async fn push_folder(folder_id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let (servers, path) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let waiting = store
            .account_owns_folder(account, folder_id)
            .map_err(|e| e.to_string())?
            && !store
                .folder_is_local(folder_id)
                .map_err(|e| e.to_string())?
            && store
                .folder_awaits_server(folder_id)
                .map_err(|e| e.to_string())?;
        if !waiting {
            return Ok(());
        }
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        let servers = store.account_servers(account).map_err(|e| e.to_string())?;
        (servers.map(|s| (account, s)), path)
    };
    // Outside the lock: the keychain may ask, and nothing should queue behind it.
    let Some(cfg) = servers.and_then(|(account, s)| imap_config_from_servers(account, s)) else {
        return Ok(());
    };
    match petrel_providers::imap::create_folder(&cfg, &path).await {
        Ok(()) => {
            let store = state.store()?;
            store
                .confirm_folder_on_server(folder_id)
                .map_err(|e| e.to_string())
        }
        Err(e) => {
            log_sync(&format!(
                "server create {path} failed, next sync retries: {e}"
            ));
            Err(e.to_string())
        }
    }
}

/// Whether a folder lives only here, so renaming or deleting it has nothing
/// to ask the server: a local one never goes there, and one still waiting
/// has not arrived. Asking anyway was refused as a mailbox that does not
/// exist, and that refusal stopped the local change as well — a folder whose
/// create had failed could be neither renamed nor deleted.
fn only_here(store: &petrel_engine::store::Store, folder_id: i64) -> Result<bool, String> {
    Ok(store
        .folder_is_local(folder_id)
        .map_err(|e| e.to_string())?
        || store
            .folder_awaits_server(folder_id)
            .map_err(|e| e.to_string())?)
}

/// Renames a folder — on the server first, then locally, so the two cannot
/// disagree with the server holding the older name.
///
/// One still waiting for the server is renamed here only, and keeps waiting:
/// the sync creates it under whatever it is called by then.
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
        let cfg = match only_here(&store, folder_id)? {
            true => None,
            false => imap_config_for(&store, account),
        };
        (cfg, path)
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
/// synced is destroyed. One that lives only here is deleted only here.
#[tauri::command]
pub async fn delete_folder(folder_id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let (cfg, path) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let path = store
            .folder_path(folder_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such folder")?;
        let cfg = match only_here(&store, folder_id)? {
            true => None,
            false => imap_config_for(&store, account),
        };
        (cfg, path)
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
            state.caps(account).has_uidplus,
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
#[tauri::command(async)]
pub fn reorder_folders(ids: Vec<i64>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.reorder_folders(&ids).map_err(|e| e.to_string())
}

/// The order somebody dragged their tags into. Local, for the same reason.
#[tauri::command(async)]
pub fn reorder_tags(ids: Vec<i64>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.reorder_tags(&ids).map_err(|e| e.to_string())
}

/// How many messages a folder holds, so a confirmation can name the number.
#[tauri::command(async)]
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
            state.caps(account).has_move,
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
