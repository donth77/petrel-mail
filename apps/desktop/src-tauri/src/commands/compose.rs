//! Writing mail: drafts, quoting, attachments being staged, scheduling, and the identity a message goes out under.

use crate::diag::data_dir;
use crate::state::{AppState, active_account, now_ms};
use crate::sync::drafts::{push_draft_to_server, schedule_draft_push, spawn_drop_server_draft};
use petrel_engine::store::{DraftRecord, Identity};
use std::sync::Arc;
use tauri::State;

/// Saves the composer's contents so they survive closing it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn save_draft(
    draft_id: Option<i64>,
    to: String,
    cc: Option<String>,
    subject: String,
    body: String,
    html: String,
    in_reply_to: Option<String>,
    references: Option<Vec<String>>,
    attachments: Option<Vec<String>>,
    state: State<Arc<AppState>>,
) -> Result<i64, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    let envelope = petrel_engine::store::DraftEnvelope {
        in_reply_to,
        references: references.unwrap_or_default(),
        attachments: attachments.unwrap_or_default(),
    };
    let id = store
        .save_draft_full(
            account,
            draft_id,
            &to,
            cc.as_deref().unwrap_or(""),
            &subject,
            &body,
            &html,
            &envelope,
        )
        .map_err(|e| e.to_string())?;
    drop(store);
    // The server copy follows on the 30-second clock; closing the composer
    // pushes at once through `push_draft` instead of waiting it out.
    schedule_draft_push(Arc::clone(state.inner()), id);
    Ok(id)
}

/// Pushes the draft's current text to the server now — the composer closing
/// is the one moment the debounce must not be allowed to lose.
#[tauri::command]
pub async fn push_draft(id: i64, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Ok(mut dirty) = state.draft_dirty.lock() {
        dirty.remove(&id);
    }
    push_draft_to_server(state.inner(), id).await
}

#[tauri::command]
pub fn load_draft(id: i64, state: State<Arc<AppState>>) -> Result<DraftRecord, String> {
    let store = state.store()?;
    let record = store.load_draft(id).map_err(|e| e.to_string())?;
    if !record.body.is_empty() || !record.html.is_empty() {
        return Ok(record);
    }
    // A draft written in another client: it arrived through folder sync as a
    // message, so its words live in the raw blob rather than in the draft
    // columns. Reconstruct the composer's view from the message itself.
    // (Attachments stay with the server copy for now — the words are what a
    // draft is; reattaching is a save away.)
    let Some(hash) = store.blob_hash_for(id).ok().flatten() else {
        return Ok(record);
    };
    let Ok(raw) = state.blobs.read(&hash) else {
        return Ok(record);
    };
    let Some(parsed) = petrel_mime::parse_message(&raw) else {
        return Ok(record);
    };
    let join = |list: &[(Option<String>, String)]| {
        list.iter()
            .map(|(_, addr)| addr.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Ok(DraftRecord {
        id,
        to: join(&parsed.to),
        cc: join(&parsed.cc),
        subject: parsed.subject.clone().unwrap_or_default(),
        body: parsed.body_text.clone(),
        html: parsed
            .body_html
            .clone()
            .unwrap_or_else(|| petrel_mime::plain_text_to_html(&parsed.body_text)),
        envelope: petrel_engine::store::DraftEnvelope {
            in_reply_to: parsed.references.last().cloned().map(|r| format!("<{r}>")),
            references: parsed.references.iter().map(|r| format!("<{r}>")).collect(),
            attachments: Vec::new(),
        },
    })
}

#[tauri::command]
pub fn delete_draft(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    // The server's copy goes with it. Read before the local row disappears.
    spawn_drop_server_draft(state.inner(), id);
    let store = state.store()?;
    store.delete_draft(id).map_err(|e| e.to_string())
}

/// Addresses to offer while a recipient is being typed.
#[tauri::command]
pub fn complete_addresses(
    prefix: String,
    state: State<Arc<AppState>>,
) -> Result<Vec<petrel_engine::store::Correspondent>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store
        .complete_addresses(account, &prefix, now_ms(), 8)
        .map_err(|e| e.to_string())
}

/// Issues a one-message URL for the reading pane. The UI never receives the
/// body over IPC — bulk bytes go over the custom protocol, and the frame that
/// renders them has no IPC access at all.
/// The original of a message, ready to be quoted in a reply.
#[derive(serde::Serialize)]
pub(crate) struct Quoted {
    html: String,
    text: String,
    from: String,
    date_ms: i64,
    /// The message's own recipients and subject, for a forward's header block.
    /// Taken from the message rather than from the conversation: a thread's
    /// subject drifts, and forwarding one message out of the middle of it
    /// should say what *that* message said.
    to: String,
    subject: String,
}

/// Reads a message back for quoting.
///
/// Sanitized before it leaves, and with remote content stripped — not because
/// the composer would render it, but because whatever is quoted is about to be
/// *sent*. Quoting a tracked message with its pixel intact would forward that
/// pixel to everyone on the reply and fire it again for each of them, turning
/// the person replying into the tracker's delivery mechanism.
#[tauri::command]
pub fn quote_message(message_id: i64, state: State<Arc<AppState>>) -> Result<Quoted, String> {
    let store = state.store()?;
    let hash = store
        .blob_hash_for(message_id)
        .map_err(|e| e.to_string())?
        .ok_or("message has no stored body")?;
    let raw = state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable")?;
    let parsed = petrel_mime::parse_message(&raw).ok_or("message could not be parsed")?;

    let (from, date_ms) = store
        .message_header(message_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();

    let html = match parsed.body_html.as_deref() {
        Some(h) => petrel_mime::sanitize_html(h, false).html,
        // No HTML half: the text becomes the quote, escaped into paragraphs so
        // it arrives as prose rather than as one run-on line.
        None => petrel_mime::plain_text_to_html(&parsed.body_text),
    };

    let to = parsed
        .to
        .iter()
        .map(|(name, addr)| match name {
            Some(n) if !n.trim().is_empty() => format!("{n} <{addr}>"),
            _ => addr.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");

    Ok(Quoted {
        html,
        text: parsed.body_text,
        from,
        date_ms,
        to,
        subject: parsed.subject.clone().unwrap_or_default(),
    })
}

/// Writes a dropped file to disk and reports where it landed.
///
/// A file picked from the dialog arrives as a path, because the dialog is the
/// system's and hands one over. A file dragged in from the desktop does not:
/// the webview gives the page bytes and deliberately withholds the path, so
/// there is nothing for the sender to open later. Staging it is what turns the
/// one into the other, and means everything downstream — the size rule, the
/// list in the composer, the send itself — keeps working on paths and does not
/// learn that drops exist.
///
/// The name is reduced to a file name and nothing else. It arrives from a drag
/// the application did not compose, so `../../.ssh/id_rsa` has to be a file
/// called `id_rsa` in the staging directory and not a path out of it.
#[tauri::command]
pub fn stage_attachment(name: String, bytes: Vec<u8>) -> Result<AttachmentInfo, String> {
    let stem = std::path::Path::new(&name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "attachment".to_string());

    let dir = data_dir().join("staged");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    // Prefixed rather than overwritten: dropping two files of the same name
    // from different folders is ordinary, and the second must not replace the
    // first after the first is already listed in the composer.
    let unique = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        stem
    );
    let path = dir.join(unique);
    let size = bytes.len() as u64;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;

    Ok(AttachmentInfo {
        name: stem,
        size,
        path: path.to_string_lossy().into_owned(),
    })
}

/// Name and size for files the user picked, so the composer can refuse an
/// oversized one before the message is written.
///
/// Statted here rather than in the window: the file picker hands back paths,
/// and asking the OS for a size is something the backend can already do
/// without a second plugin and a second capability to review.
#[tauri::command]
pub fn attachment_info(paths: Vec<String>) -> Vec<AttachmentInfo> {
    paths
        .into_iter()
        .map(|path| {
            let p = std::path::Path::new(&path);
            AttachmentInfo {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone()),
                // Unreadable reports zero rather than failing the whole pick;
                // the send will report it properly if it is still a problem.
                size: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                path,
            }
        })
        .collect()
}

#[derive(serde::Serialize)]
pub(crate) struct AttachmentInfo {
    path: String,
    name: String,
    size: u64,
}

/// A content type from the file extension.
///
/// Deliberately a short list plus a catch-all. Guessing wrong is harmless —
/// application/octet-stream always works and every client offers to save it —
/// whereas a large mapping table is a lot of lines that can only be subtly
/// wrong. The types here are the ones people actually attach.
pub(crate) fn guess_content_type(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        // The method the reply means is in the file's own METHOD line; the
        // content type is what makes calendar systems look inside at all.
        "ics" => "text/calendar",
        "zip" => "application/zip",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Marks a draft to go later, or pulls it back.
#[tauri::command]
pub fn schedule_send(
    draft_id: i64,
    at_ms: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    store
        .schedule_send(draft_id, at_ms)
        .map_err(|e| e.to_string())?;
    // Wake the worker: a message due in the past should not wait for the next
    // poll just because it was scheduled after the fact.
    state.drain_signal.notify_one();
    Ok(())
}

/// Who mail is sent as, and what goes underneath it.
#[tauri::command]
pub fn get_identity(state: State<Arc<AppState>>) -> Result<Identity, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store.identity(account).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_identity(
    display_name: String,
    signature: String,
    signature_on_reply: bool,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store
        .set_identity(account, &display_name, &signature, signature_on_reply)
        .map_err(|e| e.to_string())
}
