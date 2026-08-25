//! Drafts on the server: pushed after a pause, dropped when sent or discarded.

use crate::config::imap_config_for;
use crate::diag::log_sync;
use crate::state::AppState;
use petrel_engine::store::Store;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Pushes one draft to the server's Drafts folder, replacing its previous
/// copy there.
///
/// The draft travels under a Message-ID minted on its first push and kept for
/// life: every later push carries the same one, so the server copy is an edit
/// rather than a sibling — and when ordinary folder sync fetches it back, the
/// dedupe key lands it on the local draft row instead of beside it. The old
/// server copy is deleted only when it is exactly the UID this store
/// recorded; a copy some other client replaced meanwhile is left standing, so
/// a conflicting revision is never silently discarded.
pub(crate) async fn push_draft_to_server(
    state: &Arc<AppState>,
    draft_id: i64,
) -> Result<(), String> {
    let (record, msgid, old_uid, cfg, drafts_path, identity, domain) = {
        let mut store = state.store()?;
        let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let record = store.load_draft(draft_id).map_err(|e| e.to_string())?;
        let (msgid, old_uid) = store
            .draft_sync_state(draft_id)
            .map_err(|e| e.to_string())?;
        let Some(cfg) = imap_config_for(&store, account) else {
            // No server to push to is not a failure of the draft.
            return Ok(());
        };
        let drafts_path = store
            .folder_for_role(account, "drafts")
            .ok()
            .flatten()
            .and_then(|fid| store.folder_path(fid).ok().flatten());
        let identity = store.identity(account).ok();
        let domain = cfg
            .user
            .split('@')
            .nth(1)
            .unwrap_or("localhost")
            .to_string();
        let msgid = match msgid {
            Some(m) => m,
            None => {
                let minted = format!(
                    "draft-{:x}.{}@{domain}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0),
                    std::process::id(),
                );
                store
                    .set_draft_msgid(draft_id, &minted)
                    .map_err(|e| e.to_string())?;
                minted
            }
        };
        (record, msgid, old_uid, cfg, drafts_path, identity, domain)
    };
    let Some(drafts_path) = drafts_path else {
        return Ok(());
    };
    let _ = domain;

    let msg = petrel_providers::smtp::Outgoing {
        from_addr: cfg.user.clone(),
        from_name: identity.map(|i| i.display_name).unwrap_or_default(),
        to: addresses_of(&record.to),
        cc: addresses_of(&record.cc),
        subject: record.subject.clone(),
        body_text: record.body.clone(),
        body_html: Some(record.html.clone()).filter(|h| !h.trim().is_empty()),
        in_reply_to: record.envelope.in_reply_to.clone(),
        references: record.envelope.references.clone(),
        // Attachment files stay local until send: a draft's paths may not
        // even exist by the time it is reopened, and pushing megabytes on
        // every autosave is the wrong trade. The text notes nothing; other
        // clients see the words, which is what a draft is.
        attachments: Vec::new(),
    };
    let raw = msg.render_with_id(&msgid);

    petrel_providers::imap::append_message(&cfg, &drafts_path, Some("(\\Draft \\Seen)"), &raw)
        .await
        .map_err(|e| format!("append: {e}"))?;
    let new_uid = petrel_providers::imap::uids_for_message_id(&cfg, &drafts_path, &msgid)
        .await
        .ok()
        .and_then(|hits| hits.last().copied());

    if let Some(old) = old_uid
        && new_uid != Some(old)
    {
        // Only the exact copy this store recorded. Anything else standing at
        // another UID is somebody's revision, and it stays.
        if let Err(e) = petrel_providers::imap::expunge_uid(
            &cfg,
            &drafts_path,
            old,
            state.server_has_uidplus.load(Ordering::Relaxed),
        )
        .await
        {
            log_sync(&format!("old draft copy (uid {old}) not removed: {e}"));
        }
    }
    // Absent (search failed), the next push simply leaves a copy behind
    // rather than deleting blind.
    if let Ok(mut store) = state.store.lock() {
        let _ = store.set_draft_server_uid(draft_id, new_uid);
    }
    log_sync(&format!("draft {draft_id} pushed to {drafts_path}"));
    Ok(())
}

/// Marks the draft dirty and, if it was clean, starts the 30-second clock.
///
/// Saves inside the window coalesce: the sleeping task pushes whatever the
/// draft says when the clock runs out, which is the newest save. Closing the
/// composer pushes immediately through the `push_draft` command instead.
pub(crate) fn schedule_draft_push(state: Arc<AppState>, draft_id: i64) {
    {
        let Ok(mut dirty) = state.draft_dirty.lock() else {
            return;
        };
        if !dirty.insert(draft_id) {
            return; // a task is already sleeping on it
        }
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let still_dirty = state
            .draft_dirty
            .lock()
            .map(|mut d| d.remove(&draft_id))
            .unwrap_or(false);
        if still_dirty && let Err(e) = push_draft_to_server(&state, draft_id).await {
            log_sync(&format!("draft {draft_id} push failed: {e}"));
        }
    });
}

/// Deletes the draft's server copy, if one was recorded — for a draft being
/// discarded, or one that just became a sent message. Reads through the
/// caller's guard, because two of the three callers already hold the lock.
pub(crate) fn drop_server_draft_using(store: &Store, draft_id: i64, uidplus: bool) {
    let Ok((_, Some(uid))) = store.draft_sync_state(draft_id) else {
        return;
    };
    let Some(account) = store.active_account().ok().flatten() else {
        return;
    };
    let Some(cfg) = imap_config_for(store, account) else {
        return;
    };
    let Some(path) = store
        .folder_for_role(account, "drafts")
        .ok()
        .flatten()
        .and_then(|fid| store.folder_path(fid).ok().flatten())
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        // UIDPLUS makes the expunge surgical; without it the fallback path
        // inside expunge_uid does the careful dance. Read fresh per call.
        if let Err(e) = petrel_providers::imap::expunge_uid(&cfg, &path, uid, uidplus).await {
            log_sync(&format!("server draft copy (uid {uid}) not removed: {e}"));
        }
    });
}

/// The lock-acquiring face of `drop_server_draft_using`.
pub(crate) fn spawn_drop_server_draft(state: &Arc<AppState>, draft_id: i64) {
    let uidplus = state.server_has_uidplus.load(Ordering::Relaxed);
    let Ok(store) = state.store.lock() else {
        return;
    };
    drop_server_draft_using(&store, draft_id, uidplus);
}

/// Splits a recipient field the way the composer's chip field does —
/// commas and semicolons — for rendering a draft whose addresses are still
/// one string. A draft may legitimately have none at all.
fn addresses_of(field: &str) -> Vec<String> {
    field
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
