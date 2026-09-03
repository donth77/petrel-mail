//! Mail on its way out: what the outbox holds and what can be done about it.

use crate::config::imap_config;
use crate::send::sent_folder_evidence;
use crate::state::{AppState, active_account, now_ms};
use crate::sync::drafts::drop_server_draft_using;
use std::sync::Arc;
use tauri::State;

/// The outbox, row by row, with each message's state.
#[tauri::command]
pub fn list_outbox(
    state: State<Arc<AppState>>,
) -> Result<Vec<petrel_engine::store::OutboxRow>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.outbox(account).map_err(|e| e.to_string())
}

/// "Send now", "Try now", "Send anyway". The person has looked and decided,
/// which is the only thing that may move a message out of `NeedsAttention` —
/// so this is also the one place that does.
#[tauri::command]
pub fn outbox_send_now(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    {
        let store = state.store()?;
        store.resend_now(id, now_ms()).map_err(|e| e.to_string())?;
    }
    // Wake the send worker so "now" means now, not the next drain.
    state.wake_send();
    Ok(())
}

/// "Edit": back to Drafts with the text intact, out of the queue.
#[tauri::command]
pub fn outbox_edit(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    store.unschedule_send(id).map_err(|e| e.to_string())
}

/// "Check again" for a message whose outcome is unknown: look in Sent once
/// more and resolve it if the evidence is now there. Never sends.
#[tauri::command]
pub async fn outbox_check(id: i64, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    use petrel_engine::outbox::{AttemptOutcome, SendState, reconcile};
    let (account, message_id) = {
        let store = state.store()?;
        let account = active_account(&store)?;
        let row = store
            .outbox(account)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("that message is no longer in the outbox")?;
        let mid: Option<String> = store
            .conn_query_send_message_id(id)
            .map_err(|e| e.to_string())?;
        (account, mid.filter(|_| row.state == "NeedsAttention"))
    };
    let Some(mid) = message_id else {
        return Ok("Indeterminate".into());
    };
    let cfg = imap_config(&state, account).ok_or("no account is configured")?;
    let evidence = sent_folder_evidence(&state, &cfg, account, &mid).await;
    let next = reconcile(AttemptOutcome::UnknownAfterTransmit, evidence);
    let store = state.store()?;
    match next {
        SendState::Sent => {
            drop_server_draft_using(state.inner(), &store, id);
            let _ = store.delete_draft(id);
        }
        SendState::RetryQueued => {
            let _ = store.resend_now(id, now_ms());
            state.wake_send();
        }
        _ => {}
    }
    Ok(format!("{next:?}"))
}
