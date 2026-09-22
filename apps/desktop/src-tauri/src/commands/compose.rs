//! Writing mail: drafts, quoting, attachments being staged, scheduling, and the identity a message goes out under.

use crate::commands::clean_header;
use crate::diag::{create_private_dir, data_dir};
use crate::state::{AppState, active_account, now_ms};
use crate::sync::drafts::{push_draft_to_server, schedule_draft_push, spawn_drop_server_draft};
use petrel_engine::store::{DraftRecord, Identity};
use std::sync::Arc;
use tauri::State;

/// Saves the composer's contents so they survive closing it.
#[tauri::command(async)]
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
    // The draft's own account, never the active one. Cmd-2 with a composer
    // open switches the rail while the message stays on screen, and every
    // save after it wrote the draft under the other account — a different
    // address to send as, a different signature, a different server's
    // Drafts folder. A draft belongs to the account it was started in until
    // somebody says otherwise.
    let account = match draft_id {
        Some(id) => store
            .account_of_message(id)
            .map_err(|e| e.to_string())?
            .ok_or("that draft is no longer here")?,
        None => active_account(&store)?,
    };
    // Files already on the row went through this same check when they were
    // attached; anything new must be a path Petrel itself handed out.
    let held: Vec<String> = match draft_id {
        Some(id) => store
            .load_draft(id)
            .map(|d| d.envelope.attachments)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let mut files = Vec::new();
    for path in attachments.unwrap_or_default() {
        files.push(
            state
                .vetted_path(&path, &held)?
                .to_string_lossy()
                .into_owned(),
        );
    }
    // Scrubbed on the way in as well as on the way out. The composer's
    // fields are ordinary text to the person typing them, but a reply's
    // subject and reply headers arrive from a message somebody else wrote —
    // and a header value carrying a newline is not a value, it is a second
    // header. Only the header fields: a body may contain whatever it likes.
    let envelope = petrel_engine::store::DraftEnvelope {
        in_reply_to: in_reply_to.map(|v| clean_header(&v)),
        references: references
            .unwrap_or_default()
            .iter()
            .map(|r| clean_header(r))
            .collect(),
        attachments: files,
    };
    let id = store
        .save_draft_full(
            account,
            draft_id,
            &clean_header(&to),
            &clean_header(cc.as_deref().unwrap_or("")),
            &clean_header(&subject),
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

/// A revision of this draft saved by another client, if the sweeps found
/// one standing beside ours on the server.
#[derive(serde::Serialize)]
pub struct DraftConflict {
    pub other_id: i64,
}

#[tauri::command(async)]
pub fn draft_conflict(
    id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<DraftConflict>, String> {
    let store = state.store()?;
    Ok(store
        .draft_conflict(id)
        .map_err(|e| e.to_string())?
        .map(|(other_id, _)| DraftConflict { other_id }))
}

/// Settles a draft conflict the way the person chose.
///
/// Take the server's: its words become the draft, its UID becomes the
/// recorded one, and our superseded copy is expunged from the server. Keep
/// this version: the other revision is expunged and a push makes the server
/// say what the composer says. Either way exactly one revision remains,
/// chosen rather than raced — the data layer never discarded either, which
/// is what makes the question askable at all.
#[tauri::command]
pub async fn resolve_draft_conflict(
    id: i64,
    other_id: i64,
    take_server: bool,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    use crate::config::imap_config_for;

    let (account, cfg, drafts_path, our_uid, other_uid) = {
        let store = state.store()?;
        // The draft's account. Expunging its server copy under whichever
        // account the rail happens to show destroys a stranger's draft.
        let account = store
            .account_of_message(id)
            .map_err(|e| e.to_string())?
            .ok_or("that draft is no longer here")?;
        let cfg = imap_config_for(&store, account);
        let drafts_path = store
            .folder_for_role(account, "drafts")
            .ok()
            .flatten()
            .and_then(|fid| store.folder_path(fid).ok().flatten());
        let (_, our_uid) = store.draft_sync_state(id).map_err(|e| e.to_string())?;
        let other_uid = store
            .draft_conflict(id)
            .map_err(|e| e.to_string())?
            .filter(|(oid, _)| *oid == other_id)
            .and_then(|(_, uid)| uid);
        (account, cfg, drafts_path, our_uid, other_uid)
    };

    if take_server {
        // The other revision's words, out of its blob.
        let (subject, body, html) = {
            let store = state.store()?;
            let hash = store
                .blob_hash_for(other_id)
                .map_err(|e| e.to_string())?
                .ok_or("the server revision has no stored body")?;
            let raw = state.blobs.read(&hash).map_err(|e| e.to_string())?;
            let parsed = petrel_mime::parse_message(&raw).ok_or("unparseable revision")?;
            (
                parsed.subject.unwrap_or_default(),
                parsed.body_text,
                parsed.body_html.unwrap_or_default(),
            )
        };
        {
            let store = state.store()?;
            store
                .adopt_server_revision(id, &subject, &body, &html, other_uid.map(|u| u as u32))
                .map_err(|e| e.to_string())?;
            store
                .retire_second_copy(other_id)
                .map_err(|e| e.to_string())?;
        }
        // Our superseded server copy goes; the adopted one stands.
        if let (Some(cfg), Some(path), Some(uid)) = (&cfg, &drafts_path, our_uid)
            && Some(uid as i64) != other_uid
        {
            let _ = petrel_providers::imap::expunge_uid(
                cfg,
                path,
                uid,
                state.caps(account).has_uidplus,
            )
            .await;
        }
        crate::diag::log_sync(&format!("draft {id}: took the server's revision"));
    } else {
        {
            let store = state.store()?;
            store
                .retire_second_copy(other_id)
                .map_err(|e| e.to_string())?;
        }
        // The other revision goes from the server too — that is what keeping
        // this version means — and a push makes the server agree.
        if let (Some(cfg), Some(path), Some(uid)) = (&cfg, &drafts_path, other_uid) {
            let _ = petrel_providers::imap::expunge_uid(
                cfg,
                path,
                uid as u32,
                state.caps(account).has_uidplus,
            )
            .await;
        }
        crate::sync::drafts::schedule_draft_push(Arc::clone(&state), id);
        crate::diag::log_sync(&format!("draft {id}: kept the local version"));
    }
    Ok(())
}

#[tauri::command(async)]
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

#[tauri::command(async)]
pub fn delete_draft(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    // The server's copy goes with it. Read before the local row disappears.
    spawn_drop_server_draft(state.inner(), id);
    let store = state.store()?;
    store.delete_draft(id).map_err(|e| e.to_string())
}

/// Addresses to offer while a recipient is being typed.
#[tauri::command(async)]
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
    /// Where the author asks replies to go, when they asked. Read from the
    /// stored bytes here rather than kept in a column: it is needed at the
    /// moment somebody hits reply and nowhere else, so it costs one parse of a
    /// message already being parsed, instead of a schema change and a pass over
    /// every message ever received.
    reply_to: Vec<String>,
}

/// Reads a message back for quoting.
///
/// Sanitized before it leaves, and with remote content stripped — not because
/// the composer would render it, but because whatever is quoted is about to be
/// *sent*. Quoting a tracked message with its pixel intact would forward that
/// pixel to everyone on the reply and fire it again for each of them, turning
/// the person replying into the tracker's delivery mechanism.
///
/// The message's own pictures are another matter: nothing is fetched to show
/// them, so they stay, written in from its parts (`embed_cid_images`). Left as
/// `cid:` references they were broken in the composer and broken on arrival.
#[tauri::command(async)]
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
        Some(h) => {
            let clean = petrel_mime::sanitize_html(h, false).html;
            petrel_mime::embed_cid_images(&paragraph_breaks_as_blank_lines(&clean), &raw)
        }
        // No HTML half: the text becomes the quote, a paragraph to a line.
        None => plain_text_quote(&parsed.body_text),
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
        reply_to: parsed
            .reply_to
            .iter()
            .map(|(_, addr)| addr.clone())
            .collect(),
    })
}

/// The original's paragraph breaks, written in as blank lines.
///
/// The composer draws a paragraph as a line with no gap under it: right for
/// what is being written, and wrong for a quoted original whose paragraphs
/// relied on a gap to stand apart. Two of them read as one block, the blank
/// line between them gone. Every mail client, Petrel's reader among them, gives
/// a `<p>` a margin, so an empty paragraph between two worded ones puts back
/// what the reader showed. Paragraphs that already have an empty one between
/// them, as Outlook writes its blank lines, keep just that one; lines written
/// as `<div>`s, as Gmail and Apple Mail write them, had no gap and get none.
///
/// Runs on sanitized HTML, which is serialized: tags are lowercase, balanced,
/// and paragraphs never nest, so a paragraph ends at the first `</p>`.
fn paragraph_breaks_as_blank_lines(html: &str) -> String {
    // Each paragraph's span, and whether it holds any words.
    let mut paragraphs: Vec<(usize, usize, bool)> = Vec::new();
    let mut from = 0;
    while let Some(rel) = html[from..].find("<p") {
        let start = from + rel;
        let after = html.as_bytes().get(start + 2).copied();
        if !matches!(after, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')) {
            from = start + 2;
            continue;
        }
        let Some(close) = html[start..].find("</p>").map(|c| start + c) else {
            break;
        };
        let end = close + "</p>".len();
        let inner = &html[start..close];
        let inner = inner.find('>').map_or("", |gt| &inner[gt + 1..]);
        paragraphs.push((start, end, has_words(inner)));
        from = end;
    }
    let mut out = String::with_capacity(html.len() + paragraphs.len() * 7);
    let mut copied = 0;
    for pair in paragraphs.windows(2) {
        let ((_, end, worded), (next, _, next_worded)) = (pair[0], pair[1]);
        if worded && next_worded && html[end..next].trim().is_empty() {
            out.push_str(&html[copied..end]);
            out.push_str("<p></p>");
            copied = end;
        }
    }
    out.push_str(&html[copied..]);
    out
}

/// Whether a paragraph's markup holds anything but tags, spaces and
/// non-breaking spaces.
fn has_words(inner: &str) -> bool {
    let mut in_tag = false;
    let mut text = String::new();
    for c in inner.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    !text
        .replace("&nbsp;", " ")
        .replace('\u{a0}', " ")
        .trim()
        .is_empty()
}

/// A plain-text original as paragraphs the composer keeps: a line each, and a
/// blank line as an empty one. The reader's markup, a `<div>` to a line, lost
/// every blank line on the way in: an empty div is nothing to the editor, so
/// the original's paragraphs ran together in the quote.
fn plain_text_quote(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                "<p></p>".to_string()
            } else {
                let escaped = line
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                format!("<p>{escaped}</p>")
            }
        })
        .collect()
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
#[tauri::command(async)]
pub fn stage_attachment(name: String, bytes: Vec<u8>) -> Result<AttachmentInfo, String> {
    stage_bytes(&name, &bytes)
}

/// Stages the original's attachments for a forward, as every client forwards
/// them. `parts` are the ones the reader lists.
///
/// The composer attaches by path, so each part becomes a file in the staging
/// directory: the one a dropped file lands in, which the send already accepts
/// and the launch sweep already clears. A picture the forwarded body shows is
/// skipped, because the body carries it (`quoted_pictures`) and attaching it as
/// well would send it twice.
#[tauri::command(async)]
pub fn stage_forwarded_attachments(
    message_id: i64,
    parts: Vec<usize>,
    state: State<Arc<AppState>>,
) -> Result<Vec<AttachmentInfo>, String> {
    let hash = state
        .store_read_open()?
        .blob_hash_for(message_id)
        .map_err(|e| e.to_string())?
        .ok_or("message has no stored body")?;
    let raw = state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable")?;
    let parsed = petrel_mime::parse_message(&raw).ok_or("message could not be parsed")?;
    // The same sanitized body the quote is built from, so the two agree on
    // which pictures travel inside it.
    let in_body: std::collections::HashSet<usize> = match parsed.body_html.as_deref() {
        Some(h) => {
            let html = petrel_mime::sanitize_html(h, false).html;
            petrel_mime::quoted_pictures(&html, &raw)
                .into_iter()
                .map(|p| p.part)
                .collect()
        }
        None => Default::default(),
    };
    let mut staged = Vec::new();
    for part in parts {
        if in_body.contains(&part) {
            continue;
        }
        let (meta, bytes) = petrel_mime::attachment_bytes(&raw, part)
            .ok_or("that attachment is not in the message")?;
        let name = super::attachments::safe_filename(meta.filename.as_deref());
        staged.push(stage_bytes(&name, &bytes)?);
    }
    Ok(staged)
}

fn stage_bytes(name: &str, bytes: &[u8]) -> Result<AttachmentInfo, String> {
    stage_bytes_in(&data_dir().join("staged"), name, bytes)
}

/// Stages a file this app wrote itself, such as an invitation reply, and
/// returns the path the send accepts.
pub(crate) fn stage_file(name: &str, bytes: &[u8]) -> Result<String, String> {
    stage_bytes(name, bytes).map(|info| info.path)
}

fn stage_bytes_in(
    root: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> Result<AttachmentInfo, String> {
    let stem = std::path::Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "attachment".to_string());

    // 0700 on Unix: a staged file is somebody's mail sitting in a directory
    // under their profile, and every other account on the machine could
    // read it.
    create_private_dir(root).map_err(|e| e.to_string())?;

    // A directory of its own for each file, and the file inside under its own
    // name. The send names an attachment after its file, and the timestamp
    // this used to put in front of the name went out with it: a dropped
    // report.pdf arrived as 1726912345678901234-report.pdf. Two files of the
    // same name still never meet.
    let dir = fresh_private_dir(root).map_err(|e| e.to_string())?;
    let path = dir.join(&stem);
    let size = bytes.len() as u64;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;

    Ok(AttachmentInfo {
        name: stem,
        size,
        path: path.to_string_lossy().into_owned(),
    })
}

/// A new, empty, private directory under `root`. Created rather than reused,
/// so one that already exists is never shared.
fn fresh_private_dir(root: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for n in 0..1000u32 {
        let dir = root.join(format!("{now}-{n}"));
        #[cfg(unix)]
        let made = {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir)
        };
        #[cfg(not(unix))]
        let made = std::fs::create_dir(&dir);
        match made {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free staging directory"))
}

/// Name and size for files the user picked, so the composer can refuse an
/// oversized one before the message is written.
///
/// Statted here rather than in the window: the file picker hands back paths,
/// and asking the OS for a size is something the backend can already do
/// without a second plugin and a second capability to review.
#[tauri::command(async)]
pub fn attachment_info(paths: Vec<String>, state: State<Arc<AppState>>) -> Vec<AttachmentInfo> {
    paths
        .into_iter()
        // Only paths a Petrel picker produced. Statting an arbitrary path
        // says whether it exists and how big it is, which is a little
        // filesystem oracle for anything that gets into the window.
        .filter_map(|path| state.vetted_path(&path, &[]).ok().map(|p| (path, p)))
        .map(|(path, p)| AttachmentInfo {
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone()),
            // Unreadable reports zero rather than failing the whole pick;
            // the send will report it properly if it is still a problem.
            size: std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
            path,
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
///
/// The row is looked up first so that scheduling a draft that is no longer
/// here is an error rather than a silent success: an UPDATE that matches
/// nothing returns Ok, and a message discarded while its composer was open
/// reported "Sending in 20 seconds" about nothing at all. The account is the
/// row's own — the send worker reads the queue per account, so the message
/// goes out over the servers of whichever account wrote it, whatever the
/// rail is showing by then.
#[tauri::command(async)]
pub fn schedule_send(
    draft_id: i64,
    at_ms: Option<i64>,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    store
        .account_of_message(draft_id)
        .map_err(|e| e.to_string())?
        .ok_or("that message is no longer here")?;
    store
        .schedule_send(draft_id, at_ms)
        .map_err(|e| e.to_string())?;
    // Wake the send worker, and the clock so it sleeps until this time
    // rather than finishing an empty-outbox nap.
    state.wake_send();
    Ok(())
}

/// Who mail is sent as, and what goes underneath it.
#[tauri::command(async)]
pub fn get_identity(state: State<Arc<AppState>>) -> Result<Identity, String> {
    let store = state.store()?;
    let account = active_account(&store)?;
    store.identity(account).map_err(|e| e.to_string())
}

#[tauri::command(async)]
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

#[cfg(test)]
mod draft_account_tests {
    use petrel_engine::store::{AccountServers, DraftEnvelope, Store};

    fn two_accounts() -> (Store, i64, i64) {
        let store = Store::open_in_memory().expect("store");
        let first = store
            .add_account("imap", "a@example.com", "A", &AccountServers::default())
            .expect("first");
        let second = store
            .add_account("imap", "b@example.com", "B", &AccountServers::default())
            .expect("second");
        (store, first, second)
    }

    /// A draft belongs to the account it was written in, and keeps belonging
    /// to it while the rail moves. Cmd-1…9 fires even while somebody is
    /// typing, so the composer outliving an account switch is ordinary use
    /// rather than an edge case.
    #[test]
    fn a_draft_keeps_its_own_account_whatever_the_rail_shows() {
        let (store, first, second) = two_accounts();
        store.set_active_account(first).unwrap();
        let draft = store
            .save_draft(first, None, "someone@example.com", "Hi", "body", "")
            .unwrap();

        // The rail moves to the other account.
        store.set_active_account(second).unwrap();
        assert_eq!(store.active_account().unwrap(), Some(second));
        // What the save reads instead of the active account.
        assert_eq!(store.account_of_message(draft).unwrap(), Some(first));

        // Saving again under that account leaves the row where it was.
        store
            .save_draft_full(
                first,
                Some(draft),
                "someone@example.com",
                "",
                "Hi again",
                "body",
                "",
                &DraftEnvelope::default(),
            )
            .unwrap();
        assert_eq!(store.account_of_message(draft).unwrap(), Some(first));

        // And the send worker reads the queue per account, so the message
        // goes out over the servers of the account that wrote it — with its
        // address, its signature, its Sent folder — and never the other's.
        store.schedule_send(draft, Some(1_000)).unwrap();
        assert!(
            store
                .due_sends(first, 2_000)
                .unwrap()
                .iter()
                .any(|d| d.id == draft),
            "the account that wrote it sends it"
        );
        assert!(
            store.due_sends(second, 2_000).unwrap().is_empty(),
            "the account on screen must not send another account's message"
        );
    }

    /// A draft discarded while its composer was still open. Both commands
    /// look the row up first, so a save or a schedule against a message that
    /// is no longer here is an error rather than an UPDATE matching nothing
    /// and reporting success.
    #[test]
    fn a_draft_that_is_gone_is_an_error_rather_than_a_silent_success() {
        let (store, first, _second) = two_accounts();
        let draft = store
            .save_draft(first, None, "someone@example.com", "Hi", "body", "")
            .unwrap();
        store.delete_draft(draft).unwrap();
        assert_eq!(
            store.account_of_message(draft).unwrap(),
            None,
            "the lookup both commands refuse on"
        );
        // The store itself is happy to schedule nothing at all, which is
        // why the check has to be here.
        assert!(store.schedule_send(draft, Some(1_000)).is_ok());
        assert!(store.due_sends(first, 2_000).unwrap().is_empty());
    }
}

#[cfg(test)]
mod staging_tests {
    use super::stage_bytes_in;

    /// The send names an attachment after its file, so a staged file carries
    /// exactly the name it was given, however many share it.
    #[test]
    fn a_staged_file_keeps_its_own_name() {
        let root = tempfile::tempdir().expect("temp dir");
        let one = stage_bytes_in(root.path(), "report.pdf", b"one").expect("first");
        let two = stage_bytes_in(root.path(), "report.pdf", b"two").expect("second");
        for info in [&one, &two] {
            let path = std::path::Path::new(&info.path);
            assert_eq!(path.file_name().expect("a file"), "report.pdf");
            assert!(path.starts_with(root.path()), "{}", info.path);
            assert_eq!(info.name, "report.pdf");
        }
        assert_ne!(one.path, two.path, "two files of one name met");
        assert_eq!(std::fs::read(&one.path).expect("read"), b"one");
        assert_eq!(std::fs::read(&two.path).expect("read"), b"two");
    }

    /// The name arrives from a drag or a message somebody else wrote, so it is
    /// a name and never a path.
    #[test]
    fn a_staged_name_cannot_climb_out() {
        let root = tempfile::tempdir().expect("temp dir");
        let info = stage_bytes_in(root.path(), "../../.ssh/id_rsa", b"x").expect("staged");
        let path = std::path::Path::new(&info.path);
        assert_eq!(path.file_name().expect("a file"), "id_rsa");
        assert!(path.starts_with(root.path()), "{}", info.path);
    }
}

#[cfg(test)]
mod quote_layout_tests {
    use super::{paragraph_breaks_as_blank_lines, plain_text_quote};

    /// Two paragraphs in the original read as two in the quote, with the blank
    /// line between them that the reader showed.
    #[test]
    fn paragraphs_keep_the_break_between_them() {
        assert_eq!(
            paragraph_breaks_as_blank_lines("<p>Friday?</p><p>Thanks,<br>Dana</p>"),
            "<p>Friday?</p><p></p><p>Thanks,<br>Dana</p>"
        );
        assert_eq!(
            paragraph_breaks_as_blank_lines("<p>One.</p>\n<p class=\"x\">Two.</p>"),
            "<p>One.</p><p></p>\n<p class=\"x\">Two.</p>"
        );
    }

    /// Outlook writes its blank lines as paragraphs of their own. Adding more
    /// beside them would double every gap.
    #[test]
    fn a_written_blank_line_is_not_doubled() {
        let outlook = "<p>Hi</p><p>&nbsp;</p><p>Thanks</p><p><br></p><p>Sam</p>";
        assert_eq!(paragraph_breaks_as_blank_lines(outlook), outlook);
    }

    /// Gmail and Apple Mail write lines as divs, with no gap to put back.
    #[test]
    fn lines_and_other_blocks_are_left_alone() {
        for html in [
            "<div>one</div><div>two</div>",
            "<div><p>a</p></div><div><p>b</p></div>",
            "<blockquote><p>a</p></blockquote><p>b</p>",
            "<pre>x</pre><p>y</p>",
        ] {
            assert_eq!(paragraph_breaks_as_blank_lines(html), html);
        }
    }

    #[test]
    fn a_plain_text_original_keeps_its_blank_lines() {
        assert_eq!(
            plain_text_quote("Hi Sam,\n\nThanks for this.\n<b> is not markup"),
            "<p>Hi Sam,</p><p></p><p>Thanks for this.</p><p>&lt;b&gt; is not markup</p>"
        );
    }
}

