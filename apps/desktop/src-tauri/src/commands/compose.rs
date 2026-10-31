//! Writing mail: drafts, quoting, attachments being staged, scheduling, and the identity a message goes out under.

use crate::commands::clean_header;
use crate::diag::{create_private_dir, data_dir};
use crate::state::{AppState, active_account, now_ms};
use crate::sync::drafts::{drop_server_draft_using, push_draft_to_server, schedule_draft_push};
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
    bcc: Option<String>,
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
    let envelope = draft_envelope(in_reply_to, references, files, bcc);
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

/// The parts of a draft that are not its text, as the store keeps them.
///
/// Scrubbed on the way in as well as on the way out. The composer's fields
/// are ordinary text to the person typing them, but a reply's subject and
/// reply headers arrive from a message somebody else wrote — and a header
/// value carrying a newline is not a value, it is a second header. Only the
/// header fields: a body may contain whatever it likes. Bcc is one: it is
/// written into the copies kept in Sent and Drafts.
fn draft_envelope(
    in_reply_to: Option<String>,
    references: Option<Vec<String>>,
    attachments: Vec<String>,
    bcc: Option<String>,
) -> petrel_engine::store::DraftEnvelope {
    petrel_engine::store::DraftEnvelope {
        in_reply_to: in_reply_to.map(|v| clean_header(&v)),
        references: references
            .unwrap_or_default()
            .iter()
            .map(|r| clean_header(r))
            .collect(),
        attachments,
        bcc: clean_header(bcc.as_deref().unwrap_or("")),
    }
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
    settle_draft_conflict(
        &state,
        id,
        other_id,
        take_server,
        crate::config::imap_config_for,
    )
    .await
}

/// `resolve_draft_conflict`, given where the account's server comes from:
/// the keychain's password in the app, a scripted server in the tests.
pub(crate) async fn settle_draft_conflict<F>(
    state: &Arc<AppState>,
    id: i64,
    other_id: i64,
    take_server: bool,
    server: F,
) -> Result<(), String>
where
    F: FnOnce(&petrel_engine::store::Store, i64) -> Option<petrel_providers::imap::ImapConfig>,
{
    let (account, cfg, drafts_path, drafts_validity, our_uid, other_uid) = {
        let store = state.store()?;
        // The draft's account. Expunging its server copy under whichever
        // account the rail happens to show destroys a stranger's draft.
        let account = store
            .account_of_message(id)
            .map_err(|e| e.to_string())?
            .ok_or("that draft is no longer here")?;
        let cfg = server(&store, account);
        // Both numbers are in Drafts, and go with the UIDVALIDITY they belong
        // to, read under the same guard: Drafts renumbered before the sync
        // noticed has given them to other drafts, and the server refuses
        // them then (review r1, R1-7).
        let drafts = store.folder_for_role(account, "drafts").ok().flatten();
        let drafts_path = drafts.and_then(|fid| store.folder_path(fid).ok().flatten());
        let drafts_validity = drafts.and_then(|fid| store.folder_validity(fid).ok().flatten());
        let (_, our_uid) = store.draft_sync_state(id).map_err(|e| e.to_string())?;
        let other_uid = store
            .draft_conflict(id)
            .map_err(|e| e.to_string())?
            .filter(|(oid, _)| *oid == other_id)
            .and_then(|(_, uid)| uid);
        (
            account,
            cfg,
            drafts_path,
            drafts_validity,
            our_uid,
            other_uid,
        )
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
                drafts_validity,
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
                drafts_validity,
                state.caps(account).has_uidplus,
            )
            .await;
        }
        crate::sync::drafts::schedule_draft_push(Arc::clone(state), id);
        crate::diag::log_sync(&format!("draft {id}: kept the local version"));
    }
    Ok(())
}

/// A draft as a composer opens it: the stored record, and each attached file's
/// name and size.
///
/// The envelope keeps paths only, so a reopened file used to come back as
/// 0 B: its chip said so, and it counted for nothing against the attachment
/// limit. The sizes are read from disk here, as they are for a file just
/// picked (`attachment_info`). These paths come from the store: the window's
/// went through `vetted_path` in `save_draft`, and the rest are files the
/// backend staged itself, so the window cannot use this to stat a path of its
/// choosing.
#[derive(serde::Serialize)]
pub struct OpenedDraft {
    #[serde(flatten)]
    record: DraftRecord,
    files: Vec<AttachmentInfo>,
}

impl OpenedDraft {
    fn new(record: DraftRecord) -> Self {
        let files = record
            .envelope
            .attachments
            .iter()
            .map(|path| file_info(path.clone(), std::path::Path::new(path)))
            .collect();
        Self { record, files }
    }
}

#[tauri::command(async)]
pub fn load_draft(id: i64, state: State<Arc<AppState>>) -> Result<OpenedDraft, String> {
    stored_draft(id, &state).map(OpenedDraft::new)
}

fn stored_draft(id: i64, state: &AppState) -> Result<DraftRecord, String> {
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
    Ok(record_from_message(id, &parsed))
}

/// The composer's view of a draft another client wrote: its recipients, Bcc
/// included, its words and the thread it answers, read from the message.
fn record_from_message(id: i64, parsed: &petrel_mime::ParsedMessage) -> DraftRecord {
    let join = |list: &[(Option<String>, String)]| {
        list.iter()
            .map(|(_, addr)| addr.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    DraftRecord {
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
            bcc: join(&parsed.bcc),
        },
    }
}

#[cfg(test)]
mod bcc_tests {
    use super::{draft_envelope, record_from_message};

    /// The composer's Bcc field is kept with the draft, scrubbed like any
    /// other header value: it is written into the copies kept in Sent and
    /// Drafts, where a newline would be a header of somebody else's choosing.
    #[test]
    fn a_drafts_blind_copies_are_kept_and_scrubbed() {
        let e = draft_envelope(
            None,
            None,
            Vec::new(),
            Some("priya@example.net,\r\nX-Injected: yes".into()),
        );
        assert_eq!(e.bcc, "priya@example.net,X-Injected: yes");
        assert_eq!(draft_envelope(None, None, Vec::new(), None).bcc, "");
    }

    /// A draft written in another client keeps its blind copies when it is
    /// opened here, read from its own Bcc header.
    #[test]
    fn a_draft_from_another_client_keeps_its_blind_copies() {
        let raw = b"From: Sam <sam@example.com>\r\nTo: dana@example.com\r\n\
Bcc: Priya Nair <priya@example.net>, board@example.org\r\n\
Subject: Numbers\r\nMessage-ID: <d1@example.com>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nDraft words\r\n";
        let parsed = petrel_mime::parse_message(raw).unwrap();
        let record = record_from_message(5, &parsed);
        assert_eq!(record.to, "dana@example.com");
        assert_eq!(record.envelope.bcc, "priya@example.net, board@example.org");
        assert_eq!(record.body.trim(), "Draft words");
    }
}

/// Discards a draft, or a message waiting in the outbox.
///
/// Not one on the wire. Deleted under the worker, it went anyway, and the
/// person who had just discarded it was told nothing: the outcome had no row
/// to land on. One guard for the look and the delete, so the worker cannot
/// claim it in between.
#[tauri::command(async)]
pub fn delete_draft(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    crate::commands::outbox::refuse_while_transmitting(&store, id)?;
    // And not a row that is no longer this message: its id reused by mail
    // that arrived since, which Discard would have deleted.
    crate::commands::outbox::refuse_when_gone(&store, id)?;
    // The server's copy goes with it. Read before the local row disappears.
    drop_server_draft_using(state.inner(), &store, id);
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
///
/// So are the remote pictures of a message whose remote content the reader
/// shows, because the person allowed it. Those are fetched once, here, and
/// written in the same way (`embed_remote_pictures`): the reply carries them
/// as its own parts, never as links, so nobody on it fires the tracker again.
#[tauri::command(async)]
pub fn quote_message(message_id: i64, state: State<Arc<AppState>>) -> Result<Quoted, String> {
    // What the quote needs from the store, and then the store let go: the
    // pictures below can take seconds to fetch, and the write lock held
    // through that would stop every click and every sync in the app.
    let (hash, (from, date_ms)) = {
        let store = state.store()?;
        let hash = store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message has no stored body")?;
        let header = store
            .message_header(message_id)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        (hash, header)
    };
    let raw = state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable")?;
    let parsed = petrel_mime::parse_message(&raw).ok_or("message could not be parsed")?;

    // The reader's rules for this message's remote content are the quote's.
    let remote = super::remote::remote_allowed(&state, message_id);
    let html = match parsed.body_html.as_deref() {
        Some(h) => {
            let clean = if remote {
                remote_quote_html(h, |url| {
                    super::remote::fetch_picture(url, REMOTE_PICTURE_CAP)
                })
            } else {
                petrel_mime::sanitize_html(h, false).html
            };
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

/// A remote picture past this is left out of a quote: the composer's own
/// paste limit (`EMBED_CAP`).
const REMOTE_PICTURE_CAP: u64 = 8 * 1024 * 1024;
/// Every remote picture in one quote, together.
const REMOTE_PICTURES_BUDGET: usize = 16 * 1024 * 1024;
/// And how many are fetched at all. A newsletter can carry hundreds.
const REMOTE_PICTURES_MAX: usize = 20;

/// The quote of a message whose remote content the reader shows.
///
/// A quote carries no remote reference at all. Its pictures are fetched and
/// written in as data (`embed_remote_pictures_with`), and then the ordinary
/// pass runs over the result: it keeps data pictures and strips whatever
/// remote reference is left, an unfetched picture, a table background, a
/// `url()` in a style. Sanitized once with remote content allowed, those
/// stayed in the quote, and a reply or forward sent without an edit (which
/// is when the editor never rewrites it) carried them to everyone on it.
fn remote_quote_html(
    original: &str,
    fetch: impl Fn(&str) -> Result<(String, Vec<u8>), String> + Sync,
) -> String {
    let with_remote = petrel_mime::sanitize_html(original, true).html;
    let embedded = embed_remote_pictures_with(&with_remote, fetch);
    petrel_mime::sanitize_html(&embedded, false).html
}

fn embed_remote_pictures_with(
    html: &str,
    fetch: impl Fn(&str) -> Result<(String, Vec<u8>), String> + Sync,
) -> String {
    // Every distinct remote `src`, in order, as the sanitizer serialized it.
    let mut urls: Vec<String> = Vec::new();
    for quote in ['"', '\''] {
        let needle = format!("src={quote}http");
        let mut from = 0;
        while let Some(rel) = html[from..].find(&needle) {
            let start = from + rel + "src=".len() + 1;
            let Some(len) = html[start..].find(quote) else {
                break;
            };
            let url = &html[start..start + len];
            if !urls.iter().any(|u| u == url) {
                urls.push(url.to_string());
            }
            from = start + len;
        }
    }
    urls.truncate(REMOTE_PICTURES_MAX);

    let fetched: Vec<Option<(String, Vec<u8>)>> = std::thread::scope(|s| {
        let fetch = &fetch;
        let running: Vec<_> = urls
            .iter()
            .map(|url| {
                // The serialized form escapes `&`; the request must not.
                let url = url.replace("&amp;", "&");
                s.spawn(move || fetch(&url).ok())
            })
            .collect();
        running
            .into_iter()
            .map(|t| t.join().ok().flatten())
            .collect()
    });

    let mut out = html.to_string();
    let mut spent = 0usize;
    for (url, got) in urls.iter().zip(fetched) {
        let Some((mime, bytes)) = got else {
            continue;
        };
        if spent + bytes.len() > REMOTE_PICTURES_BUDGET {
            continue;
        }
        spent += bytes.len();
        let data = petrel_mime::data_url(&mime, &bytes);
        for quote in ['"', '\''] {
            out = out.replace(
                &format!("src={quote}{url}{quote}"),
                &format!("src=\"{data}\""),
            );
        }
    }
    // Whatever is still remote was refused, failed, or did not fit. Out of
    // `src`, so the composer leaves it out rather than sending a link.
    out.replace("src=\"http", "data-remote=\"http")
        .replace("src='http", "data-remote='http")
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
/// Nor do paragraphs whose own style takes the gap away, as Petrel's sent
/// mail does (`flush_sides`).
///
/// Runs on sanitized HTML, which is serialized: tags are lowercase, balanced,
/// and paragraphs never nest, so a paragraph ends at the first `</p>`.
fn paragraph_breaks_as_blank_lines(html: &str) -> String {
    // Each paragraph's span, whether it holds any words, and whether its
    // style leaves no space above it and below it.
    let mut paragraphs: Vec<(usize, usize, bool, (bool, bool))> = Vec::new();
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
        let (open, inner) = html[start..close].split_once('>').unwrap_or(("", ""));
        paragraphs.push((start, end, has_words(inner), flush_sides(open)));
        from = end;
    }
    let mut out = String::with_capacity(html.len() + paragraphs.len() * 7);
    let mut copied = 0;
    for pair in paragraphs.windows(2) {
        let ((_, end, worded, (_, flush_below)), (next, _, next_worded, (flush_above, _))) =
            (pair[0], pair[1]);
        // The reader's gap is the larger of the two margins that meet here,
        // and the padding on either side of them.
        let gap = !(flush_below && flush_above);
        if worded && next_worded && gap && html[end..next].trim().is_empty() {
            out.push_str(&html[copied..end]);
            out.push_str("<p></p>");
            copied = end;
        }
    }
    out.push_str(&html[copied..]);
    out
}

/// Whether a paragraph's inline style leaves no space above it, and none
/// below it: its margin and its padding on that side both draw nothing.
///
/// Petrel sends every paragraph with `margin:0` (`zero_paragraph_margins` in
/// petrel-providers), which the sanitizer writes back as `margin: 0`, so the
/// reader shows its own mail as lines. Padding counts as well, because it
/// does not collapse: an email template zeroes the margin and spaces its
/// paragraphs with `padding-bottom`, and the reader shows that gap. A
/// paragraph's margin is the reader's gap until its style says otherwise; its
/// padding is nothing until its style says otherwise. Declarations are read
/// in order and the last one wins, as in CSS.
fn flush_sides(open_tag: &str) -> (bool, bool) {
    let Some((_, style)) = open_tag.split_once(" style=\"") else {
        return (false, false);
    };
    let style = style.split('"').next().unwrap_or("");
    let (mut margin, mut padding) = ((false, false), (true, true));
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let sides: Vec<&str> = value.split_whitespace().collect();
        let first = sides.first().copied();
        // One or two values give the top's to the bottom too; three or four
        // name the bottom third.
        let both = (
            draws_nothing(first),
            draws_nothing(sides.get(2).copied().or(first)),
        );
        match property.trim() {
            "margin" => margin = both,
            "margin-top" => margin.0 = draws_nothing(first),
            "margin-bottom" => margin.1 = draws_nothing(first),
            "padding" => padding = both,
            "padding-top" => padding.0 = draws_nothing(first),
            "padding-bottom" => padding.1 = draws_nothing(first),
            _ => {}
        }
    }
    (margin.0 && padding.0, margin.1 && padding.1)
}

/// Whether a CSS length draws nothing: under half a pixel, which a screen
/// cannot show. Outlook writes its lines as `margin-bottom: .0001pt`. A unit
/// not known here counts only at zero.
fn draws_nothing(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let value = value.to_ascii_lowercase();
    let unit_at = value
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(unit_at);
    let Ok(number) = number.parse::<f32>() else {
        return false;
    };
    // CSS's own sizes for the absolute units, and the reader's 16px text
    // for the ones that follow the font.
    let pixels = match unit {
        "" | "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" => 96.0 / 101.6,
        "em" | "rem" => 16.0,
        "ex" | "ch" => 8.0,
        _ => return number == 0.0,
    };
    (number * pixels).abs() < 0.5
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
        .map(|(path, p)| file_info(path, &p))
        .collect()
}

/// A file's name and size, as its chip shows them: `path` as the window
/// names it, read at `on_disk`. Unreadable reports zero rather than failing
/// the whole pick or the whole draft; the send will report it properly if it
/// is still a problem.
fn file_info(path: String, on_disk: &std::path::Path) -> AttachmentInfo {
    AttachmentInfo {
        name: on_disk
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone()),
        size: std::fs::metadata(on_disk).map(|m| m.len()).unwrap_or(0),
        path,
    }
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

/// Writes the name and signature of the account the Identities pane was
/// opened on, not whichever the rail shows by the time a keystroke saves: a
/// switch under the open pane wrote one account's name over the other's.
#[tauri::command(async)]
pub fn set_identity(
    account_id: i64,
    display_name: String,
    signature: String,
    signature_on_reply: bool,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let store = state.store()?;
    let account = crate::commands::triage::named_account(&store, account_id)?;
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
mod opened_draft_tests {
    use super::OpenedDraft;
    use petrel_engine::store::{DraftEnvelope, DraftRecord};

    fn with_files(attachments: Vec<String>) -> DraftRecord {
        DraftRecord {
            id: 7,
            to: String::new(),
            cc: String::new(),
            subject: String::new(),
            body: "body".into(),
            html: String::new(),
            envelope: DraftEnvelope {
                attachments,
                ..DraftEnvelope::default()
            },
        }
    }

    /// A reopened file comes back with its size, so its chip does not say
    /// 0 B and it counts against the limit. One that has gone since keeps
    /// its place at zero rather than failing the draft.
    #[test]
    fn a_reopened_draft_knows_how_big_its_files_are() {
        let root = tempfile::tempdir().expect("temp dir");
        let here = root.path().join("report.pdf");
        std::fs::write(&here, vec![0u8; 2048]).expect("write");
        let gone = root.path().join("gone.pdf");
        let paths = vec![
            here.to_string_lossy().into_owned(),
            gone.to_string_lossy().into_owned(),
        ];

        let opened = OpenedDraft::new(with_files(paths.clone()));

        let files: Vec<_> = opened
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.name.as_str(), f.size))
            .collect();
        assert_eq!(
            files,
            [
                (paths[0].as_str(), "report.pdf", 2048),
                (paths[1].as_str(), "gone.pdf", 0),
            ]
        );
    }

    /// The window reads the record's fields where they always were, with the
    /// files beside them.
    #[test]
    fn an_opened_draft_is_the_record_with_its_files() {
        let json = serde_json::to_value(OpenedDraft::new(with_files(Vec::new()))).expect("json");
        assert_eq!(json["id"], 7);
        assert_eq!(json["body"], "body");
        assert!(json["envelope"]["attachments"].is_array());
        assert_eq!(json["files"], serde_json::json!([]));
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

    /// docs/24 #19. Petrel sends every paragraph with no margin, so the reader
    /// shows its mail as lines, and a quote of it is lines too. A blank line
    /// between each pair double-spaced every reply to your own mail, and the
    /// round after that quoted the blank lines as well.
    #[test]
    fn petrels_own_lines_stay_lines_in_a_quote() {
        let sent = petrel_providers::smtp::Outgoing {
            from_addr: "sam@example.com".into(),
            from_name: "Sam".into(),
            to: vec!["dana@example.com".into()],
            cc: vec![],
            bcc: vec![],
            subject: "Friday".into(),
            body_text: "Line one\nLine two\nLine three".into(),
            body_html: Some("<p>Line one</p><p>Line two</p><p>Line three</p>".into()),
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        };
        let (_, raw) = sent.render("example.com");
        let parsed = petrel_mime::parse_message(&raw).unwrap();
        let shown = petrel_mime::sanitize_html(parsed.body_html.as_deref().unwrap(), false).html;
        assert!(shown.contains("Line three"), "{shown}");
        assert_eq!(
            paragraph_breaks_as_blank_lines(&shown),
            shown,
            "the lines the reader shows together stay together in the quote"
        );
    }

    /// The gap a reader shows between two paragraphs is the larger of the
    /// first one's bottom margin and the second one's top margin, so a style
    /// takes it away only when both are zero.
    #[test]
    fn a_gap_the_reader_shows_is_still_a_blank_line() {
        for (html, quoted) in [
            // Beside an ordinary paragraph, whose margin is the gap.
            (
                r#"<p style="margin: 0">One.</p><p>Two.</p>"#,
                r#"<p style="margin: 0">One.</p><p></p><p>Two.</p>"#,
            ),
            // A margin below, set after the zero: the later one wins, as in CSS.
            (
                r#"<p style="margin: 0; margin-bottom: 12px">One.</p><p style="margin: 0">Two.</p>"#,
                r#"<p style="margin: 0; margin-bottom: 12px">One.</p><p></p><p style="margin: 0">Two.</p>"#,
            ),
            // The shorthand's third value is the bottom.
            (
                r#"<p style="margin: 0 0 1em">One.</p><p style="margin: 0">Two.</p>"#,
                r#"<p style="margin: 0 0 1em">One.</p><p></p><p style="margin: 0">Two.</p>"#,
            ),
        ] {
            assert_eq!(paragraph_breaks_as_blank_lines(html), quoted);
        }
        // Zero in any unit, and the sides left alone, is still no gap.
        let tight = r#"<p style="margin: 0px auto">One.</p><p style="margin-top: 0; margin-bottom: 0in">Two.</p>"#;
        assert_eq!(paragraph_breaks_as_blank_lines(tight), tight);
    }

    /// Review r2, R2-2. A paragraph meets the next flush only where its
    /// margin and its padding are both nothing, since padding does not
    /// collapse: an email template zeroes the margin and spaces its paragraphs
    /// with padding, and the reader shows that gap. And a length under half a
    /// pixel draws nothing: Outlook's `margin-bottom: .0001pt` is no gap.
    /// Read through the parser and the sanitizer, as a quote is.
    #[test]
    fn padding_is_a_gap_and_a_hair_is_none() {
        for (who, body, gap) in [
            (
                "template, spaced by the padding shorthand",
                "<p style=\"margin:0;padding:0 0 16px 0\">One.</p>\n\
                 <p style=\"margin:0;padding:0 0 16px 0\">Two.</p>",
                true,
            ),
            (
                "template, spaced by padding-bottom",
                "<p style=\"margin:0;padding-bottom:16px;line-height:24px\">One.</p>\n\
                 <p style=\"margin:0;padding-bottom:16px;line-height:24px\">Two.</p>",
                true,
            ),
            (
                "padding above the second",
                "<p style=\"margin:0\">One.</p><p style=\"margin:0;padding-top:12px\">Two.</p>",
                true,
            ),
            (
                "Outlook, margin 0in then .0001pt",
                "<p class=MsoNormal style='margin:0in;margin-bottom:.0001pt'>One<o:p></o:p></p>\n\
                 <p class=MsoNormal style='margin:0in;margin-bottom:.0001pt'>Two<o:p></o:p></p>",
                false,
            ),
            (
                "a hair of padding",
                "<p style=\"margin:0;padding:0.2px 0\">One.</p><p style=\"margin:0\">Two.</p>",
                false,
            ),
            (
                "Apple's lines",
                "<p style=\"margin: 0px; font: 12px Helvetica; min-height: 14px;\">One</p>\n\
                 <p style=\"margin: 0px; font: 12px Helvetica; min-height: 14px;\">Two</p>",
                false,
            ),
        ] {
            let raw = format!(
                "From: a@example.com\r\nTo: b@example.com\r\nSubject: t\r\nMIME-Version: 1.0\r\n\
                 Content-Type: text/html; charset=utf-8\r\n\r\n<html><body>{body}</body></html>\r\n"
            );
            let parsed = petrel_mime::parse_message(raw.as_bytes()).unwrap();
            let shown =
                petrel_mime::sanitize_html(parsed.body_html.as_deref().unwrap(), false).html;
            let quoted = paragraph_breaks_as_blank_lines(&shown);
            assert_eq!(
                quoted.contains("<p></p>"),
                gap,
                "{who}: the reader shows {}a gap, and the quote says otherwise: {shown}",
                if gap { "" } else { "no " }
            );
        }
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

#[cfg(test)]
mod remote_picture_tests {
    use super::{REMOTE_PICTURES_MAX, embed_remote_pictures_with, remote_quote_html};
    use std::sync::Mutex;

    /// A quote carries no remote reference: each picture arrives as data, or
    /// is left out.
    #[test]
    fn remote_pictures_arrive_as_data_or_not_at_all() {
        let html = concat!(
            r#"<p>Hi</p><img src="https://cdn.example/a.png">"#,
            r#"<img src="http://cdn.example/b.png?x=1&amp;y=2">"#,
            r#"<img src="https://cdn.example/a.png">"#,
            r#"<img src="https://refused.example/c.png">"#,
        );
        let asked = Mutex::new(Vec::new());
        let out = embed_remote_pictures_with(html, |url| {
            asked.lock().unwrap().push(url.to_string());
            if url.contains("refused") {
                Err("refused".into())
            } else {
                Ok(("image/png".into(), vec![1, 2, 3]))
            }
        });
        let mut asked = asked.into_inner().unwrap();
        asked.sort();
        assert_eq!(
            asked,
            vec![
                "http://cdn.example/b.png?x=1&y=2",
                "https://cdn.example/a.png",
                "https://refused.example/c.png",
            ],
            "each asked once, with the ampersand as sent"
        );
        assert_eq!(out.matches("src=\"data:image/png;base64,AQID\"").count(), 3);
        assert!(out.contains(r#"<img data-remote="https://refused.example/c.png">"#));
        assert!(!out.contains("src=\"http"), "{out}");
    }

    /// What the quote hands the composer holds no remote reference of any
    /// kind: a reply sent without an edit goes out exactly as quoted.
    #[test]
    fn a_quote_keeps_its_fetched_pictures_and_no_other_remote_reference() {
        let original = concat!(
            r#"<p>Hi <a href="https://example.com/page">a link</a></p>"#,
            r#"<img src="https://cdn.example/shown.png">"#,
            r#"<img src="https://cdn.example/refused.png">"#,
            r#"<table background="https://cdn.example/bg.png"><tr><td>cell</td></tr></table>"#,
            r#"<div style="background-image: url(https://cdn.example/css.png)">styled</div>"#,
        );
        let out = remote_quote_html(original, |url| {
            if url.ends_with("shown.png") {
                Ok(("image/png".into(), vec![1, 2, 3]))
            } else {
                Err("refused".into())
            }
        });
        assert!(out.contains("data:image/png;base64,AQID"), "{out}");
        assert!(
            !out.contains("cdn.example"),
            "a remote reference survived: {out}"
        );
        // A link is somewhere to go, not something that loads.
        assert!(out.contains("https://example.com/page"), "{out}");
    }

    /// A newsletter can carry hundreds; only so many are asked for.
    #[test]
    fn only_so_many_remote_pictures_are_fetched() {
        let html: String = (0..REMOTE_PICTURES_MAX + 5)
            .map(|i| format!(r#"<img src="https://cdn.example/{i}.png">"#))
            .collect();
        let asked = Mutex::new(0usize);
        let out = embed_remote_pictures_with(&html, |_| {
            *asked.lock().unwrap() += 1;
            Ok(("image/gif".into(), vec![0]))
        });
        assert_eq!(asked.into_inner().unwrap(), REMOTE_PICTURES_MAX);
        assert_eq!(out.matches("data:image/gif").count(), REMOTE_PICTURES_MAX);
        assert_eq!(out.matches("data-remote=").count(), 5);
    }
}
