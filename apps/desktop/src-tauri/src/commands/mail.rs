//! Reading mail: the conversation list, one conversation, search, and the views' counts.

use crate::message_view::message_origin;
use crate::state::{AppState, Timed, active_account, note_ui_touch};
use petrel_engine::store::{ListView, TagSummary, ThreadIndexRow, ThreadListing, ThreadMessage};
use std::sync::Arc;
use tauri::{Manager, State};

/// The list shows conversations, not messages — the count chip is the thread
/// size (docs 06). Flags are rolled up across the thread by the engine.
#[tauri::command(async)]
pub fn list_threads(
    view: Option<String>,
    offset: u32,
    limit: u32,
    sort: Option<String>,
    ascending: Option<bool>,
    before: Option<(i64, i64)>,
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
    let store = state.store_read()?;
    // A hundred is the page the list asks for. The UI used to ask for 500
    // and the cap was 2000; both held the store lock for a mailbox-sized
    // amount of work while the window tried to scroll.
    let limit = limit.min(100);
    match before {
        Some((d, k)) => store.list_threads_after(&view, limit, sort, d, k),
        None => store.list_threads(&view, offset, limit, sort),
    }
    .map_err(|e| e.to_string())
}

/// Asks the server about one mailbox the person just opened.
///
/// Those folders are not on the IDLE wake path. Waiting for the
/// five-minute sweep is what made the list look stale next to the
/// inbox. One folder, not every folder — a wake that swept the tree
/// put the inbox behind half a minute again. Inbox, archive, tags,
/// snoozed and outbox are ignored here: inbox has IDLE, archive is
/// All Mail, the rest have no server folder to SELECT.
#[tauri::command]
pub async fn sync_mailbox(view: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let account = {
        let store = state.store()?;
        active_account(&store)?
    };
    crate::sync::spawn_view_sync(Arc::clone(state.inner()), account, &view);
    Ok(())
}

/// Names the mailbox on screen, whatever it is, so its folder can be watched
/// the way the inbox is.
///
/// Separate from `sync_mailbox`, which is a click: this follows the view
/// however it changed — an account switch, a launch, a deleted folder
/// sending the window back to the inbox — and a view with no folder behind
/// it is news too, because it is what stops the old folder being watched.
#[tauri::command]
pub async fn watch_mailbox(view: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let account = {
        let store = state.store()?;
        active_account(&store)?
    };
    let next = Some((account, view));
    // Only a real change wakes the watchers: the window says this on every
    // account epoch, and an EXAMINE per repeat would be spent on nothing.
    state.open_view.send_if_modified(|open| {
        if *open == next {
            return false;
        }
        *open = next;
        true
    });
    Ok(())
}

/// One conversation by id, for a window that was opened onto it.
///
/// Separate from `list_threads` because a popped-out window has an id and no
/// view: it cannot say which mailbox to look in, and guessing is what made it
/// claim that starred and archived conversations no longer existed.
#[tauri::command(async)]
pub fn thread_by_id(
    thread_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<ThreadListing>, String> {
    let store = state.store_read()?;
    store.thread_by_id(thread_id).map_err(|e| e.to_string())
}

/// The messages of one conversation, for the reading pane.
///
/// Paged like the list: a hundred is the IPC cap, fifty is what the pane
/// asks for. Without a limit the old path returned every message and a
/// twenty-thousand-message thread froze the window.
#[tauri::command(async)]
pub fn thread_detail(
    thread_id: i64,
    limit: Option<u32>,
    before_date_ms: Option<i64>,
    before_id: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<Vec<ThreadMessage>, String> {
    let _t = Timed::new("thread_detail");
    note_ui_touch(&state);
    let store = state.store_read()?;
    let limit = limit.unwrap_or(50).min(100);
    let before = match (before_date_ms, before_id) {
        (Some(d), Some(k)) => Some((d, k)),
        _ => None,
    };
    store
        .thread_detail_page(thread_id, Some(limit), before)
        .map_err(|e| e.to_string())
}

/// Slim cards for one conversation: sender, snippet, date. No recipients,
/// no attachments, no IPC page cap — a long thread is still one SELECT.
#[tauri::command]
pub async fn thread_index(
    thread_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ThreadIndexRow>, String> {
    super::off_runtime(state, move |state| {
        let _t = Timed::new("thread_index");
        note_ui_touch(&state);
        let store = state.store_read_index()?;
        store.thread_index(thread_id).map_err(|e| e.to_string())
    })
    .await
}

/// One message, hydrated. The pane calls this when a card is opened.
#[tauri::command]
pub async fn thread_message(
    message_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<ThreadMessage>, String> {
    super::off_runtime(state, move |state| {
        let _t = Timed::new("thread_message");
        note_ui_touch(&state);
        let store = state.store_read_open()?;
        store.thread_message(message_id).map_err(|e| e.to_string())
    })
    .await
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
#[tauri::command(async)]
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
    let store = state.store_read_counts()?;
    store.view_counts(&modes).map_err(|e| e.to_string())
}

/// Every conversation in a view, counted — not the loaded window's length.
#[tauri::command(async)]
pub fn view_count(view: Option<String>, state: State<Arc<AppState>>) -> Result<i64, String> {
    let _t = Timed::new("view_count");
    note_ui_touch(&state);
    let view = ListView::parse(view.as_deref().unwrap_or("inbox"));
    let store = state.store_read_counts()?;
    store.conversations_in(&view).map_err(|e| e.to_string())
}

/// `sort` absent means best match — the order the ranking produced, which is
/// the one thing a list cannot offer because a list has nothing to be relevant
/// to. Any other value is the same key a list would take.
#[tauri::command]
pub async fn search_messages(
    query: String,
    sort: Option<String>,
    ascending: Option<bool>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ThreadListing>, String> {
    super::off_runtime(state, move |state| {
        let _t = Timed::new("search");
        note_ui_touch(&state);
        let sort = sort.map(|key| petrel_engine::store::Sort {
            key: petrel_engine::store::SortKey::parse(&key),
            ascending: ascending.unwrap_or(false),
        });
        let store = state.store_read()?;
        store
            .search_threads_sorted(&query, 200, sort)
            .map_err(|e| e.to_string())
    })
    .await
}

/// A one-message URL for the reading pane, spelled for this platform's
/// webview — see `message_origin` for why the spelling varies.
#[tauri::command]
pub async fn message_url(
    message_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    super::off_runtime(state, move |state| {
        let _t = Timed::new("message_url");
        let store = state.store_read_open()?;
        match store.blob_hash_for(message_id).map_err(|e| e.to_string())? {
            Some(_) => Ok(format!(
                "{}/message/{}",
                message_origin(),
                state.tokens.issue(message_id)
            )),
            None => Err("message has no stored body".into()),
        }
    })
    .await
}

/// Opens a message in its own window as a printable page.
///
/// A window rather than printing the app: the app window is chrome around a
/// sandboxed frame, and printing it prints the chrome. The print window
/// loads the message's printable document over the same protocol, so the
/// same sanitizer, the same CSP and the same remote-content policy govern
/// what lands on paper — and the page opens straight into the print dialog.
/// The header labels the printable page shows, from the same `.ftl` files the
/// rest of the interface uses. See [`SourceStrings`] for why they travel this
/// way rather than being looked up where the page is built.
#[derive(serde::Deserialize, serde::Serialize, Default)]
pub struct PrintStrings {
    from: String,
    to: String,
    cc: String,
    date: String,
}

#[tauri::command(async)]
pub fn print_message(
    message_id: i64,
    strings: PrintStrings,
    app: tauri::AppHandle,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let token = {
        let store = state.store_read_open()?;
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
    let url: tauri::Url = format!("{}/print/{token}", message_origin())
        .parse()
        .map_err(|e| format!("{e}"))?;
    let said = serde_json::to_string(&strings).unwrap_or_else(|_| "{}".into());
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title("Print")
        .initialization_script(format!("window.__PETREL_PRINT__ = {said};"))
        // The printed page is a top-level document: nothing it links to may
        // load in its place. Its own script swallows clicks; this is the
        // webview refusing whatever gets past that.
        .on_navigation(crate::print_navigation_allowed)
        // Wide enough for the sheet the print document draws — a 174mm column
        // plus its padding — rather than exactly the old 700px, which left the
        // preview with no margin at all and measured a page wider than paper.
        .inner_size(772.0, 900.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens a message's stored bytes in their own window.
///
/// The reading pane shows a sanitized, parsed body; this shows what was
/// actually ingested. When the two disagree — a folded subject, a charset, a
/// blank line that did not survive — the source is the only way to tell which
/// layer lost it, and today the only route to it is exporting a whole mailbox
/// to mbox and opening the file elsewhere.
///
/// Over the message protocol like the printable page, for the same reason:
/// bulk bytes do not belong on the IPC channel, and the token keeps the URL
/// unguessable and scoped to one message. The window is our chrome around
/// escaped text under a policy that loads nothing and runs nothing — the blob
/// is hostile input and is never parsed as a document here.
/// The words the source window shows, handed over by the window that opens it.
///
/// They come from the same `.ftl` files the rest of the interface does. The page
/// is served by the message protocol and has no way to ask for them itself — it
/// has no IPC, deliberately — so they travel in the window's own init script.
/// `bytes` and `bytes_capped` arrive as patterns with a `{n}`, `{shown}` and
/// `{total}` in them, because only the page knows how large the message is.
#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SourceStrings {
    title: String,
    copy: String,
    copied: String,
    copy_manual: String,
    bytes: String,
    bytes_capped: String,
}

#[tauri::command(async)]
pub fn view_message_source(
    message_id: i64,
    strings: SourceStrings,
    app: tauri::AppHandle,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let (token, theme, accent) = {
        let store = state.store_read_open()?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message has no stored body")?;
        // Read here rather than sent by the window, so the setting and the
        // page cannot disagree, and so a window opened from anywhere gets it.
        // "system" is left off: the page then resolves it the way the app's
        // own tokens do, through prefers-color-scheme.
        let settings = store.settings().ok();
        let theme = settings
            .as_ref()
            .and_then(|s| s.get("theme").cloned())
            .filter(|t| t == "dark" || t == "light");
        // The window builds the app's dark ground itself, and that ground
        // follows the accent. Stored with a leading `#`, which would start a
        // fragment, so it is dropped here and the page checks what arrives.
        let accent = settings
            .as_ref()
            .and_then(|s| s.get("accent").cloned())
            .map(|a| a.trim_start_matches('#').to_string())
            .filter(|a| a.len() == 6 && a.bytes().all(|b| b.is_ascii_hexdigit()));
        (state.tokens.issue(message_id), theme, accent)
    };
    let label = format!("source-{message_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }
    let query = {
        let parts: Vec<String> = theme
            .iter()
            .map(|t| format!("theme={t}"))
            .chain(accent.iter().map(|a| format!("accent={a}")))
            .collect();
        match parts.is_empty() {
            true => String::new(),
            false => format!("?{}", parts.join("&")),
        }
    };
    let url: tauri::Url = format!("{}/source/{token}{query}", message_origin())
        .parse()
        .map_err(|e| format!("{e}"))?;
    // Serialised rather than interpolated by hand: a translation is text
    // somebody else wrote, and text somebody else wrote does not get pasted
    // into a script. `textContent` is all the page does with it.
    let said = serde_json::to_string(&strings).unwrap_or_else(|_| "{}".into());
    let init = format!("window.__PETREL_SOURCE__ = {said};");
    let title = if strings.title.is_empty() {
        "Message source".to_string()
    } else {
        strings.title.clone()
    };
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        // Not the subject: a window title is chrome the platform may record,
        // and no part of a message's text needs to be in it.
        .title(title)
        .initialization_script(&init)
        // Same fence as the print window. Nothing this page names may load in
        // its place, and there is nothing in it that should try.
        .on_navigation(crate::print_navigation_allowed)
        .inner_size(820.0, 900.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Writes one message's stored bytes to a path the person chose.
///
/// The exact bytes, and that is the point of it existing beside the source
/// view: that window renders a *reading* of the message, with anything not
/// valid UTF-8 written as an escape so a charset can be checked. This is the
/// message itself — the thing another client will open, a test will be built
/// from, and a report can carry.
///
/// Same shape as saving an attachment, and the same rule: the panel is opened
/// by `pick_save_path` and only a path that came back from it is accepted, so
/// the window never names a destination of its own.
#[tauri::command(async)]
pub fn save_message_eml(
    message_id: i64,
    path: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let target = state.vetted_path(&path, &[])?;
    let raw = {
        let store = state.store_read_open()?;
        let hash = store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message has no stored body")?;
        state
            .blobs
            .read(&hash)
            .map_err(|_| "message body unavailable (failed verification)".to_string())?
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "message.eml".into());
    // The name, not the path: this string reaches the window and the log.
    std::fs::write(&target, raw).map_err(|e| format!("could not write {name}: {e}"))?;
    Ok(())
}

/// A filename to suggest for one message, from its subject.
///
/// The subject, because that is what a person recognises in a folder later, and
/// it is what every other client suggests. Put through the same scrubber an
/// attachment's name is: a subject is sender-controlled text and has no more
/// business deciding a path than a filename does.
#[tauri::command(async)]
pub fn eml_filename(message_id: i64, state: State<Arc<AppState>>) -> Result<String, String> {
    let store = state.store_read_open()?;
    let subject = store
        .thread_message(message_id)
        .map_err(|e| e.to_string())?
        .map(|m| m.subject)
        .unwrap_or_default();
    let trimmed = subject.trim();
    let stem = if trimmed.is_empty() {
        "message"
    } else {
        trimmed
    };
    let cleaned = super::attachments::safe_filename(Some(stem));
    // The scrubber keeps an extension it recognises; a subject is not a
    // filename, so whatever it ended in is part of the name and .eml goes on.
    Ok(format!("{cleaned}.eml"))
}

/// Tags for the rail. Comes from the account, not from whatever rows happen to
/// be loaded — a tag with no conversation in the current page still exists.
#[tauri::command(async)]
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
