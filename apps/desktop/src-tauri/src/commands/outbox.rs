//! Mail on its way out: what the outbox holds and what can be done about it.

use crate::config::imap_config;
use crate::send::sent_folder_evidence;
use crate::state::{AppState, now_ms};
use crate::sync::drafts::drop_server_draft_using;
use std::sync::Arc;
use tauri::State;

/// The outbox, row by row, with each message's state.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
///
/// Refused while the message is actually on the wire. `unschedule_send`
/// clears the schedule whatever state the row is in, so pressing Edit
/// during the second the SMTP conversation takes cleared the row from under
/// the send worker — which then finished, wrote its outcome to a row that no
/// longer had a schedule, and left a message that had been sent sitting in
/// Drafts as if it had not.
#[tauri::command(async)]
pub fn outbox_edit(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    if outbox_state(&store, id)?.as_deref() == Some("Transmitting") {
        return Err("that message is being sent right now".into());
    }
    store.unschedule_send(id).map_err(|e| e.to_string())
}

/// The state of one outbox row, by its own account — never the active one,
/// which may be a different mailbox by the time a queued message is touched.
fn outbox_state(store: &petrel_engine::store::Store, id: i64) -> Result<Option<String>, String> {
    let Some(account) = store.account_of_message(id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(store
        .outbox(account)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.id == id)
        .map(|r| r.state))
}

/// "Check again" for a message whose outcome is unknown: look in Sent once
/// more and resolve it if the evidence is now there. Never sends.
#[tauri::command]
pub async fn outbox_check(id: i64, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    use petrel_engine::outbox::{AttemptOutcome, SendState, reconcile};
    let (account, message_id) = {
        let store = state.store()?;
        // The row's own account. Asked of the active one, a message queued
        // in the other mailbox reported that it was no longer in the outbox.
        let account = store
            .account_of_message(id)
            .map_err(|e| e.to_string())?
            .ok_or("that message is no longer here")?;
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

#[cfg(test)]
mod outbox_tests {
    use super::outbox_state;
    use petrel_engine::outbox::SendState;
    use petrel_engine::store::{AccountServers, Store};

    /// Edit and Send-now read the row's state, and the row belongs to the
    /// account that wrote it rather than to whichever one is on screen.
    #[test]
    fn a_transmitting_row_is_recognised_and_found_by_its_own_account() {
        let store = Store::open_in_memory().unwrap();
        let first = store
            .add_account("imap", "a@example.com", "A", &AccountServers::default())
            .unwrap();
        let second = store
            .add_account("imap", "b@example.com", "B", &AccountServers::default())
            .unwrap();
        // The rail shows the first account; the draft is the second's.
        store.set_active_account(first).unwrap();
        let draft = store
            .save_draft(second, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.schedule_send(draft, Some(1_771_803_000_000)).unwrap();

        assert_eq!(
            outbox_state(&store, draft).unwrap().as_deref(),
            Some("RetryQueued"),
            "a queued row is found through the account that owns it"
        );

        store
            .set_send_state(draft, SendState::Transmitting, None, None, None)
            .unwrap();
        assert_eq!(
            outbox_state(&store, draft).unwrap().as_deref(),
            Some("Transmitting"),
            "and Edit has something to refuse on"
        );

        assert_eq!(outbox_state(&store, 9_999).unwrap(), None);
    }
}
