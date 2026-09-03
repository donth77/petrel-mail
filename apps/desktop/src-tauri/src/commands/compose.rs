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
}

/// Reads a message back for quoting.
///
/// Sanitized before it leaves, and with remote content stripped — not because
/// the composer would render it, but because whatever is quoted is about to be
/// *sent*. Quoting a tracked message with its pixel intact would forward that
/// pixel to everyone on the reply and fire it again for each of them, turning
/// the person replying into the tracker's delivery mechanism.
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
#[tauri::command(async)]
pub fn stage_attachment(name: String, bytes: Vec<u8>) -> Result<AttachmentInfo, String> {
    let stem = std::path::Path::new(&name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "attachment".to_string());

    let dir = data_dir().join("staged");
    // 0700 on Unix: a staged file is somebody's mail sitting in a directory
    // under their profile, and every other account on the machine could
    // read it.
    create_private_dir(&dir).map_err(|e| e.to_string())?;

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
