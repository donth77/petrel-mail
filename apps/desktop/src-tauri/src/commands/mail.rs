//! Reading mail: the conversation list, one conversation, search, and the views' counts.

use crate::state::{AppState, Timed, note_ui_touch};
use petrel_engine::store::{ListView, TagSummary, ThreadListing, ThreadMessage};
use std::sync::Arc;
use tauri::{Manager, State};

/// The list shows conversations, not messages — the count chip is the thread
/// size (docs 06). Flags are rolled up across the thread by the engine.
#[tauri::command]
pub fn list_threads(
    view: Option<String>,
    offset: u32,
    limit: u32,
    sort: Option<String>,
    ascending: Option<bool>,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadListing>, String> {
    let _t = Timed::new("list_threads");
    note_ui_touch(&state);
    // The rail key is parsed by the engine, which owns the mapping from a view
    // to a query. An absent view means the inbox.
    let view = ListView::parse(view.as_deref().unwrap_or("inbox"));
    // Absent means the default, which is newest first: a caller that does not
    // care about order should not have to name one.
    let sort = petrel_engine::store::Sort {
        key: petrel_engine::store::SortKey::parse(sort.as_deref().unwrap_or("date")),
        ascending: ascending.unwrap_or(false),
    };
    let store = state.store()?;
    store
        .list_threads(&view, offset, limit.min(2000), sort)
        .map_err(|e| e.to_string())
}

/// One conversation by id, for a window that was opened onto it.
///
/// Separate from `list_threads` because a popped-out window has an id and no
/// view: it cannot say which mailbox to look in, and guessing is what made it
/// claim that starred and archived conversations no longer existed.
#[tauri::command]
pub fn thread_by_id(
    thread_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<ThreadListing>, String> {
    let store = state.store()?;
    store.thread_by_id(thread_id).map_err(|e| e.to_string())
}

/// The messages of one conversation, for the reading pane.
#[tauri::command]
pub fn thread_detail(
    thread_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadMessage>, String> {
    let _t = Timed::new("thread_detail");
    note_ui_touch(&state);
    let store = state.store()?;
    store.thread_detail(thread_id).map_err(|e| e.to_string())
}

/// The numbers beside the rail's mailboxes.
///
/// The mode comes from the caller rather than from stored settings because the
/// setting lives in the renderer with the rest of them, and a second copy in
/// the engine is a second thing to keep in step.
/// One entry per mailbox the person has an opinion about — `inbox`, `starred`,
/// and the rest, plus `folders` for every folder they made. Anything absent
/// falls to the engine's own rule for that mailbox, so a fresh install sends
/// nothing and still gets sensible numbers.
#[tauri::command]
pub fn view_counts(
    modes: std::collections::HashMap<String, String>,
    state: State<Arc<AppState>>,
) -> Result<Vec<(String, i64)>, String> {
    let _t = Timed::new("view_counts");
    note_ui_touch(&state);
    let modes: std::collections::HashMap<String, petrel_engine::store::CountMode> = modes
        .iter()
        .map(|(k, v)| (k.clone(), petrel_engine::store::CountMode::parse(v)))
        .collect();
    let store = state.store()?;
    store.view_counts(&modes).map_err(|e| e.to_string())
}

/// Every conversation in a view, counted — not the loaded window's length.
#[tauri::command]
pub fn view_count(view: Option<String>, state: State<Arc<AppState>>) -> Result<i64, String> {
    let _t = Timed::new("view_count");
    note_ui_touch(&state);
    let view = ListView::parse(view.as_deref().unwrap_or("inbox"));
    let store = state.store()?;
    store.conversations_in(&view).map_err(|e| e.to_string())
}

/// `sort` absent means best match — the order the ranking produced, which is
/// the one thing a list cannot offer because a list has nothing to be relevant
/// to. Any other value is the same key a list would take.
#[tauri::command]
pub fn search_messages(
    query: String,
    sort: Option<String>,
    ascending: Option<bool>,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadListing>, String> {
    let _t = Timed::new("search");
    note_ui_touch(&state);
    let sort = sort.map(|key| petrel_engine::store::Sort {
        key: petrel_engine::store::SortKey::parse(&key),
        ascending: ascending.unwrap_or(false),
    });
    let store = state.store()?;
    store
        .search_threads_sorted(&query, 200, sort)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn message_url(message_id: i64, state: State<Arc<AppState>>) -> Result<String, String> {
    let store = state.store()?;
    match store.blob_hash_for(message_id).map_err(|e| e.to_string())? {
        Some(_) => Ok(format!(
            "petrel-msg://localhost/message/{}",
            state.tokens.issue(message_id)
        )),
        None => Err("message has no stored body".into()),
    }
}

/// Opens a message in its own window as a printable page.
///
/// A window rather than printing the app: the app window is chrome around a
/// sandboxed frame, and printing it prints the chrome. The print window
/// loads the message's printable document over the same protocol, so the
/// same sanitizer, the same CSP and the same remote-content policy govern
/// what lands on paper — and the page opens straight into the print dialog.
#[tauri::command]
pub fn print_message(
    message_id: i64,
    app: tauri::AppHandle,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let token = {
        let store = state.store()?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message has no stored body")?;
        state.tokens.issue(message_id)
    };
    let label = format!("print-{message_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }
    let url: tauri::Url = format!("petrel-msg://localhost/print/{token}")
        .parse()
        .map_err(|e| format!("{e}"))?;
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title("Print")
        // Wide enough for the sheet the print document draws — a 174mm column
        // plus its padding — rather than exactly the old 700px, which left the
        // preview with no margin at all and measured a page wider than paper.
        .inner_size(772.0, 900.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Tags for the rail. Comes from the account, not from whatever rows happen to
/// be loaded — a tag with no conversation in the current page still exists.
#[tauri::command]
/// `account` names one explicitly; absent it means the account on screen. See
/// the note on `list_folders`.
pub fn list_tags(
    account: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<Vec<TagSummary>, String> {
    let store = state.store()?;
    let account = match account {
        Some(id) => id,
        None => match store.active_account().map_err(|e| e.to_string())? {
            Some(id) => id,
            None => return Ok(Vec::new()),
        },
    };
    store.tags_for_account(account).map_err(|e| e.to_string())
}
