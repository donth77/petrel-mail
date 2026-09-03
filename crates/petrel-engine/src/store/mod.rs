//! The metadata + search store: one SQLite database (WAL), fat rows, and FTS5
//! indexes over a dedicated extracted-text table (`fts_content`).
//!
//! Invariant: `fts_content` is written in the same transaction as its message
//! row, and only through this API. Index consistency is verifiable at any time
//! via [`Store::fts_integrity_check`] and repairable via [`Store::rebuild_fts`].

use crate::retention::RetentionMode;
use rusqlite::OptionalExtension;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Arc;

mod actions;
mod drafts;
mod folders;
mod listing;
mod maintenance;
mod search;

pub const SCHEMA_VERSION: i64 = 24;
/// Bumped whenever text extraction changes; a mismatch forces reindexing.
pub const EXTRACTOR_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("fts integrity check failed: {0}")]
    Integrity(String),
    #[error("ingest failed: {0}")]
    Ingest(String),
    /// A caller asked for something the store will not do — as distinct from
    /// something that went wrong while doing it.
    #[error("{0}")]
    Rejected(String),
    /// Writing an export failed. Distinct from a database error because the
    /// thing to tell the user is different: a full disk or an unwritable
    /// folder is theirs to fix.
    #[error("could not write the export: {0}")]
    Export(#[from] std::io::Error),
}

/// What a garbage-collection pass destroyed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcReport {
    pub messages_purged: usize,
    pub blobs_removed: usize,
    /// Queued actions that structurally cannot deliver, renamed out of the
    /// queue — rows predating the action_messages table.
    pub actions_orphaned: usize,
}

/// Outcome of ingesting one raw message.
#[derive(Debug, Clone)]
pub struct Ingested {
    pub message_id: i64,
    pub blob_hash: String,
    /// False when this was a re-ingest of a message already stored (a resync).
    pub was_new: bool,
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
}

/// Minimal insertable message for the M0 store spike; the MIME pipeline will
/// replace this with parsed structures.
pub struct NewMessage {
    pub account_id: i64,
    pub date_ms: i64,
    pub from_addr: String,
    pub from_display: String,
    pub to_addr: String,
    pub subject: String,
    pub body_text: String,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub message_id: i64,
    /// Raw FTS5 bm25() value: LOWER is better. Normalize before any fusion.
    pub rank: f64,
    pub snippet: String,
}

/// One conversation as the message list shows it: the newest message's
/// details, plus how many messages it stands for.
/// IMAP system flags (RFC 3501 §2.3.2), stored as a bitfield on `messages.flags`.
/// Named after the wire values rather than the UI words: the server owns these,
/// and "starred" is our word for `\Flagged`, not a separate concept.
pub mod flags {
    pub const SEEN: i64 = 1 << 0;
    pub const ANSWERED: i64 = 1 << 1;
    pub const FLAGGED: i64 = 1 << 2;
    pub const DRAFT: i64 = 1 << 3;
    pub const DELETED: i64 = 1 << 4;
}

/// Where an account's mail lives, as discovered or as typed. Stored on the
/// account row; the password is the keychain's.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AccountServers {
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// The sign-in name, which is usually the address but not always.
    pub username: String,
    /// "Gmail", "Namecheap Private Email" — what the account was set up as.
    #[serde(default)]
    pub provider: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountSummary {
    pub id: i64,
    pub kind: String,
    pub email: String,
    pub display_name: String,
    pub color: String,
    /// True when server deletions do not remove local content (Q24).
    pub local_archive: bool,
    pub message_count: i64,
    pub unread_count: i64,
    /// Newest message we hold, as a stand-in for "last synced" until the sync
    /// engine records its own timestamp.
    pub newest_ms: Option<i64>,
    pub folders: Vec<FolderMapping>,
    /// Whether this is the one the window shows.
    pub active: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderMapping {
    /// The SPECIAL-USE role: archive, sent, drafts, spam, trash.
    pub role: String,
    pub path: String,
}

/// What mending a renumbered folder found. See `remap_folder_after_reset`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RemapOutcome {
    /// Placements whose new UID was learned from the Message-ID match.
    pub rematched: usize,
    /// Placements dropped because a complete listing proved the folder no
    /// longer holds them. Message rows and blobs are untouched.
    pub dropped: usize,
    /// Server UIDs the store could not match: download these in full.
    pub to_fetch: Vec<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Attachment {
    pub filename: String,
    pub size: i64,
    /// Which part of the message this is, for fetching its bytes on demand.
    pub part: i64,
    /// The declared type — what decides whether it can be previewed in place.
    pub mime: String,
}

/// One conversation line for the reading pane: enough to draw a collapsed
/// card. Recipients, attachments and the body stay off this row so a
/// twenty-thousand-message thread is one SELECT, not one hydrate per id.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadIndexRow {
    pub id: i64,
    pub from_display: String,
    pub from_addr: String,
    pub snippet: String,
    pub date_ms: i64,
    pub unread: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadMessage {
    pub id: i64,
    pub from_display: String,
    pub from_addr: String,
    pub subject: String,
    pub snippet: String,
    pub date_ms: i64,
    pub unread: bool,
    /// To, display names — the reading pane's "to" line.
    pub to: Vec<String>,
    /// Cc, display names. Empty when the message copied nobody.
    pub cc: Vec<String>,
    /// Display names, To then Cc — "to Sam Ortiz, Dana Wu". Reply-all still
    /// walks this combined list so original To people are not left off.
    pub recipients: Vec<String>,
    /// The same people as addresses, for replying to them. Kept separate
    /// because a reply-all built from display names sends to nobody.
    pub recipient_addrs: Vec<String>,
    pub attachments: Vec<Attachment>,
    /// A calendar part is aboard — the reader asks for the invitation then,
    /// and only then; most mail never pays for the question.
    pub has_calendar: bool,
    /// The recorded answer to an invitation: accepted, tentative, declined.
    pub invite_response: Option<String>,
}

/// Somebody worth offering while a recipient is typed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Correspondent {
    pub addr: String,
    pub display: String,
    /// Whether the user has written to them, which the list shows: "you have
    /// emailed this person" is the single most useful thing to know about a
    /// suggestion, and it is also what decides the ordering.
    pub written_to: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub colour: String,
    pub thread_count: i64,
}

/// One message's worth of work waiting to reach the server.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub action_id: i64,
    pub kind_json: String,
    /// Shared per action, not copied onto every `action_messages` row.
    /// A 22k-message mark_read used to clone a multi-megabyte undo JSON
    /// once per row and hold tens of gigabytes for the length of a drain.
    pub payload_json: Arc<str>,
    pub message_id: i64,
    /// Absent when the message was never placed in a folder — nothing to
    /// address on the server, so nothing that can be delivered.
    pub uid: Option<u32>,
    pub folder_path: String,
    /// The message's Message-ID header, when it has one: the address of last
    /// resort. With no UID, a drain can still ask a server which of its
    /// numbers carries this header — the question UIDVALIDITY recovery asks,
    /// scoped to one message.
    pub msgid: Option<String>,
    /// The folders worth asking, best guess first: where the message stood
    /// when the action was queued, then wherever a placement still holds it.
    /// Empty whenever a UID is already known — nothing needs asking.
    pub candidate_paths: Vec<String>,
}

/// `Thu Jan  1 00:00:00 1970` — the shape mbox readers expect on a From line.
fn format_asctime(ms: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Civil-from-days (Howard Hinnant's algorithm), so this needs no date crate
    // and no timezone database — mbox From lines are conventionally UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    let dow = (days + 4).rem_euclid(7) as usize;
    format!(
        "{} {} {:>2} {:02}:{:02}:{:02} {}",
        DAYS[dow],
        MONTHS[(month - 1) as usize],
        d,
        h,
        m,
        s,
        year
    )
}

/// A draft, as the composer needs it back.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DraftRecord {
    pub id: i64,
    pub to: String,
    pub cc: String,
    pub subject: String,
    /// Plain text: the snippet, the search index, and the text half of the
    /// message that goes out.
    pub body: String,
    /// The rich-text half, empty for a draft written before there was one.
    pub html: String,
    /// What threads a reply into its conversation, and what is attached.
    pub envelope: DraftEnvelope,
}

/// The parts of an outgoing message that are not its text.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DraftEnvelope {
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    /// Paths on this machine. A draft is local; the file is read when it goes.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// Who a message is sent as.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub address: String,
    pub display_name: String,
    pub signature: String,
    pub signature_on_reply: bool,
}

/// What the Storage pane reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StorageReport {
    pub messages: i64,
    pub attachments: i64,
    /// The SQLite file, including its write-ahead log — the WAL is often the
    /// larger of the two after a big sync, and leaving it out reports a
    /// mailbox as smaller than it is on disk.
    pub database_bytes: u64,
    pub blob_bytes: u64,
    pub index_bytes: u64,
    /// Each account's share, in account order. The database and index are one
    /// file each and cannot be split, so those figures have no per-account
    /// counterpart.
    pub accounts: Vec<AccountStorage>,
}

/// One account's share of what is on this Mac.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountStorage {
    pub account_id: i64,
    pub messages: i64,
    /// Bytes of the blobs this account's messages and attachments point at.
    /// A message two accounts both hold is in both their figures, so these
    /// can sum to more than the total: each row answers "how much of this is
    /// mine", and that answer does not shrink because someone else has a copy.
    pub blob_bytes: u64,
}

/// A folder as the move picker shows it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FolderSummary {
    pub id: i64,
    /// Empty for folders the user made; otherwise archive, sent, trash and so on.
    pub role: String,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThreadRowTag {
    /// Carried so a row can be untagged without consulting the rail's list.
    /// Without it the only way to name a tag to the engine was to look it up
    /// by name in that list — and a tag missing from the list for any reason
    /// became one the reader could see on the message and could not remove.
    pub id: i64,
    pub name: String,
    pub colour: String,
}

/// One message in the outbox, as the view shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OutboxRow {
    pub id: i64,
    pub subject: String,
    pub to: String,
    pub send_after_ms: i64,
    /// `petrel_engine::outbox::SendState`, by name.
    pub state: String,
    pub error: Option<String>,
    pub attempts: i64,
    pub next_ms: Option<i64>,
    pub attachments: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadListing {
    pub thread_id: i64,
    /// Newest message in the conversation — what the row displays.
    pub id: i64,
    pub from_display: String,
    pub from_addr: String,
    pub subject: String,
    pub snippet: String,
    pub date_ms: i64,
    pub message_count: i64,
    pub participants: String,
    /// Thread-level rollups: a conversation is unread if *any* message in it is,
    /// which is what makes a reply to a read thread pull it back to attention.
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    /// Every tag on any message in the conversation, deduplicated.
    pub tags: Vec<ThreadRowTag>,
    /// Why this row matched, when it came from a search: the text around the
    /// hit, with the matched words wrapped in `[` and `]`.
    ///
    /// None for an ordinary list. A search result that shows the same opening
    /// line as every other row cannot say what it was answering — the reason it
    /// is there is exactly the part the reader needs.
    pub match_snippet: Option<String>,
    /// First attachment filename, for the row chip; the reader lists them all.
    pub attachment_name: Option<String>,
}

/// A list-ready row shared by recents and search results (UI surfaces).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Listing {
    pub id: i64,
    pub from_display: String,
    pub from_addr: String,
    pub subject: String,
    pub snippet: String,
    pub date_ms: i64,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF      // hiragana + katakana
        | 0x3400..=0x4DBF    // CJK ext A
        | 0x4E00..=0x9FFF    // CJK unified
        | 0xF900..=0xFAFF    // CJK compat
        | 0xAC00..=0xD7AF) // hangul
}

/// SQLite hands the row's tags back as a JSON array; a malformed or absent
/// value means "no tags", never an error — a row must render even if its tag
/// join went wrong.
fn parse_row_tags(json: Option<String>) -> Vec<ThreadRowTag> {
    let Some(j) = json else { return Vec::new() };
    match serde_json::from_str::<Vec<ThreadRowTag>>(&j) {
        Ok(tags) => tags,
        Err(e) => {
            // An empty list is the right thing to show a reader rather than
            // failing the whole listing over a chip. But it is the wrong thing
            // to do quietly: a query that stopped selecting one of the fields
            // landed here and every row came back untagged, with nothing to
            // distinguish it from a row that genuinely has no tags. In a debug
            // build that is a bug in the SQL, and it should stop the tests.
            debug_assert!(false, "row tags did not parse: {e}: {j}");
            Vec::new()
        }
    }
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// Space-separates CJK characters so `unicode61` emits one token per character.
/// Non-CJK text passes through untouched, so a mixed subject still tokenises its
/// Latin words normally. This is what makes 1- and 2-character CJK queries
/// matchable at all: the built-in trigram tokenizer matches nothing shorter than
/// three characters ([07 §5.3]).
fn cjk_spaced(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 2);
    for c in s.chars() {
        if is_cjk(c) {
            out.push(' ');
            out.push(c);
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

fn quote_token(w: &str) -> String {
    format!("\"{}\"", w.replace('"', "\"\""))
}

/// The CJK counterpart to `match_expr`: each run of CJK characters becomes a
/// *phrase* of single-character tokens, so `東京` matches only where the two
/// characters are adjacent — the precision a per-character index would otherwise
/// lose. Latin words in the same query are quoted as ordinary tokens.
fn cjk_match_expr(query: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut run: Vec<char> = Vec::new();
    let mut word = String::new();

    fn flush_run(run: &mut Vec<char>, parts: &mut Vec<String>) {
        if !run.is_empty() {
            let spaced: Vec<String> = run.iter().map(|c| c.to_string()).collect();
            parts.push(format!("\"{}\"", spaced.join(" ")));
            run.clear();
        }
    }
    fn flush_word(word: &mut String, parts: &mut Vec<String>) {
        if word.chars().any(char::is_alphanumeric) {
            parts.push(quote_token(word));
        }
        word.clear();
    }

    for c in query.chars().filter(|c| !c.is_control()) {
        if is_cjk(c) {
            flush_word(&mut word, &mut parts);
            run.push(c);
        } else if c.is_whitespace() {
            flush_run(&mut run, &mut parts);
            flush_word(&mut word, &mut parts);
        } else {
            flush_run(&mut run, &mut parts);
            word.push(c);
        }
    }
    flush_run(&mut run, &mut parts);
    flush_word(&mut word, &mut parts);

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// First contiguous CJK run in the query — what a snippet should highlight.
fn first_cjk_run(query: &str) -> String {
    let mut run = String::new();
    for c in query.chars() {
        if is_cjk(c) {
            run.push(c);
        } else if !run.is_empty() {
            break;
        }
    }
    run
}

/// Snippets come from the original text in `fts_content`, never from the
/// space-separated index copy, which would render with gaps between every
/// character.
/// The row preview: the opening of a message, on one line.
///
/// Shared by ingest and by the re-extraction repair, because two copies of
/// "the first 160 characters, whitespace collapsed" is how a repaired row ends
/// up subtly different from a freshly ingested one.
fn preview_of(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

/// What wraps a match inside a snippet.
///
/// Private-use codepoints rather than punctuation, because every printable
/// character is something a message might legitimately contain — and one that
/// appears in mail turns the sender's own text into a false highlight.
pub const MARK_START: char = '\u{E000}';
pub const MARK_END: char = '\u{E001}';

fn cjk_snippet(body: &str, query: &str) -> String {
    const PAD: usize = 24;
    let needle = first_cjk_run(query);
    let chars: Vec<char> = body.chars().collect();
    let hit = if needle.is_empty() {
        None
    } else {
        body.find(&needle).map(|b| body[..b].chars().count())
    };
    let Some(at) = hit else {
        return chars.iter().take(96).collect();
    };
    let n = needle.chars().count();
    let start = at.saturating_sub(PAD);
    let end = (at + n + PAD).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..at]);
    // The same private-use markers the FTS path uses: brackets are ordinary
    // text in mail, and two snippet sources must mark matches the same way or
    // the renderer needs to know which produced it.
    out.push(MARK_START);
    out.extend(&chars[at..at + n]);
    out.push(MARK_END);
    out.extend(&chars[at + n..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Build a safe FTS5 MATCH expression: every token is a quoted phrase (internal
/// quotes doubled), bare tokens AND together, and the final bare token becomes a
/// prefix query for as-you-type search. User text never reaches MATCH unquoted.
fn match_expr(query: &str, prefix_last: bool) -> Option<String> {
    #[derive(PartialEq)]
    enum Tok {
        Phrase(String),
        Word(String),
    }
    let mut toks: Vec<Tok> = Vec::new();
    // Control characters (incl. NUL) break FTS5's own string parser even when
    // quoted — map them to token boundaries before anything else.
    let cleaned: String = query
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut chars = cleaned.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut s = String::new();
            for ch in chars.by_ref() {
                if ch == '"' {
                    break;
                }
                s.push(ch);
            }
            if !s.trim().is_empty() {
                toks.push(Tok::Phrase(s));
            }
        } else {
            let mut s = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() || ch == '"' {
                    break;
                }
                s.push(ch);
                chars.next();
            }
            if !s.is_empty() {
                toks.push(Tok::Word(s));
            }
        }
    }
    if toks.is_empty() {
        return None;
    }
    let esc = |s: &str| s.replace('"', "\"\"");
    let last = toks.len() - 1;
    let parts: Vec<String> = toks
        .iter()
        .enumerate()
        .map(|(i, t)| match t {
            Tok::Phrase(p) => format!("\"{}\"", esc(p)),
            Tok::Word(w) => {
                if prefix_last && i == last && w.chars().count() >= 2 {
                    format!("\"{}\"*", esc(w))
                } else {
                    format!("\"{}\"", esc(w))
                }
            }
        })
        .collect();
    Some(parts.join(" "))
}

/// Assigns `row_id` to a conversation and returns the thread id.
///
/// Reference links win; a distinctive subject is a narrow fallback. When a
/// message links two previously separate threads, they are merged — the common
/// case where the middle of a conversation arrives after its ends.
fn assign_thread(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    row_id: i64,
    msgid: Option<&str>,
    references: &[String],
    subject_norm: &str,
    date_ms: i64,
) -> Result<i64> {
    {
        let mut ins = tx.prepare_cached(
            "INSERT OR IGNORE INTO message_refs(message_id, ref_msgid) VALUES (?1, ?2)",
        )?;
        for r in references {
            ins.execute(params![row_id, r])?;
        }
    }

    let mut candidates: std::collections::BTreeSet<i64> = Default::default();

    // Ancestors: messages this one replies to.
    if !references.is_empty() {
        let placeholders = std::iter::repeat_n("?", references.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT DISTINCT thread_id FROM messages
             WHERE account_id = ?1 AND thread_id IS NOT NULL
               AND message_id_hdr IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let mut args: Vec<&dyn rusqlite::ToSql> = vec![&account_id];
        for r in references {
            args.push(r);
        }
        let rows = stmt.query_map(args.as_slice(), |r| r.get::<_, i64>(0))?;
        for t in rows {
            candidates.insert(t?);
        }
    }

    // Descendants: messages already stored that reply to this one.
    if let Some(id) = msgid {
        let mut stmt = tx.prepare_cached(
            "SELECT DISTINCT m.thread_id FROM message_refs r
             JOIN messages m ON m.id = r.message_id
             WHERE r.ref_msgid = ?1 AND m.account_id = ?2 AND m.thread_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![id, account_id], |r| r.get::<_, i64>(0))?;
        for t in rows {
            candidates.insert(t?);
        }
    }

    // Subject fallback, only for subjects distinctive enough to be evidence and
    // only within a window. A wrong merge hides mail inside an unrelated
    // conversation, where nobody thinks to look — worse than an extra thread.
    if candidates.is_empty() && crate::threading::subject_is_threadable(subject_norm) {
        let window = crate::threading::SUBJECT_THREAD_WINDOW_MS;
        let mut stmt = tx.prepare_cached(
            "SELECT thread_id FROM messages
             WHERE account_id = ?1 AND subject_norm = ?2 AND thread_id IS NOT NULL
               AND abs(date_ms - ?3) <= ?4
             ORDER BY abs(date_ms - ?3) LIMIT 1",
        )?;
        if let Ok(t) = stmt.query_row(params![account_id, subject_norm, date_ms, window], |r| {
            r.get::<_, i64>(0)
        }) {
            candidates.insert(t);
        }
    }

    let thread_id = match candidates.iter().next().copied() {
        Some(target) => {
            // Merge every other candidate into the lowest id, so thread
            // identity is stable regardless of arrival order.
            for other in candidates.iter().skip(1) {
                tx.execute(
                    "UPDATE messages SET thread_id = ?1 WHERE thread_id = ?2",
                    params![target, other],
                )?;
            }
            target
        }
        // A conversation of one, named after itself.
        None => row_id,
    };

    tx.execute(
        "UPDATE messages SET thread_id = ?2, subject_norm = ?3 WHERE id = ?1",
        params![row_id, thread_id, subject_norm],
    )?;
    Ok(thread_id)
}

/// A conversation belongs in the inbox listing unless it has been filed away.
///
/// Stated as "not in archive, trash or spam" rather than "in the inbox" on
/// purpose. Mail the sync has not placed anywhere yet is still mail, and a
/// positive test would hide all of it — the same class of bug as joining on a
/// NULL `thread_id`. Search deliberately does *not* use this: archived mail is
/// exactly what people search for.
fn in_inbox(alias: &str) -> String {
    // Stated positively: the message holds an inbox placement. The first
    // form was "not filed anywhere else", and it survived until All Mail
    // sync existed — on Gmail every message is in All Mail (the archive
    // role), so the moment the All Mail walk claimed a message, "filed
    // elsewhere" became true of the entire inbox and the view emptied
    // itself, message by message, while the walk ran. Membership is the
    // fact the servers actually maintain: arriving grants the placement,
    // and archiving, binning or moving away takes it — on both kinds of
    // provider. The bin check stays as a belt: mail a sweep marks junk
    // must drop out even if a stale inbox placement lingers.
    format!(
        "EXISTS (SELECT 1 FROM placements p
                 JOIN folders f ON f.id = p.folder_id
                 WHERE p.message_id = {alias}.id AND f.role = 'inbox')
         AND NOT EXISTS (SELECT 1 FROM placements p
                         JOIN folders f ON f.id = p.folder_id
                         WHERE p.message_id = {alias}.id
                           AND f.role IN ('trash','spam'))"
    )
}

/// Which conversations the list is showing.
///
/// Parsed in the engine rather than at the IPC boundary so the mapping from a
/// rail key to a query is one thing with one set of tests, and so an unknown
/// view can never be turned into SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListView {
    /// Everything that has not been filed away. The default.
    Inbox,
    /// Flagged mail wherever it lives — except trash and spam, which are
    /// where things go to be forgotten.
    Starred,
    /// A server folder by its role: archive, sent, drafts, spam, trash.
    Folder(String),
    /// A folder the user made, addressed by row id — user folders have no
    /// role, and a path makes a poor wire key (it can hold any character).
    UserFolder(i64),
    /// Put aside until a time that has not arrived yet.
    Snoozed,
    /// Written and waiting to go.
    Outbox,
    Tag(String),
    /// Every message in the account, wherever it sits — the export's scope,
    /// not a view anyone navigates to. Trash and Spam are in it deliberately:
    /// "everything" that quietly left some of it out would be the wrong
    /// promise, and the exported folder header says what each message is, so
    /// whoever reads the file can leave out whatever they like.
    All,
}

/// What the numbers beside the rail's mailboxes count.
///
/// Unread by default, because the question a mailbox usually has to answer is
/// "is there anything here for me". Total suits anyone who wants the rail to
/// say how big each mailbox is, and off suits anyone who finds counts noisy —
/// this is a client that would rather not badge people at things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountMode {
    Unread,
    Total,
    Off,
}

/// What a list is ordered by, and which way.
///
/// Date is the one the store is built for: conversations are paged by walking
/// an index newest-first and stopping when the page is full, so a mailbox of
/// any size costs about a page. The other two have no such index — a
/// conversation's sender and subject are its *newest message's*, which is not
/// known until the conversation has been resolved — so they group first and
/// sort afterwards. Measured on a real 26,000-message account: date 6ms,
/// sender 139ms. Fine for a deliberate change of sort, which is what this is;
/// it is not the path every list open takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Date,
    Sender,
    Subject,
}

impl SortKey {
    pub fn parse(s: &str) -> SortKey {
        match s {
            "sender" => SortKey::Sender,
            "subject" => SortKey::Subject,
            _ => SortKey::Date,
        }
    }
}

/// A list's order: what by, and which way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    /// Ascending means oldest first, or A to Z. Newest first is the default
    /// because that is what a mailbox is for.
    pub ascending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort {
            key: SortKey::Date,
            ascending: false,
        }
    }
}

/// The rail's fixed mailboxes, in the order they ship in.
///
/// One list, because the sidebar section that reorders and hides them, the
/// counts query, and the settings that store somebody's arrangement all have
/// to agree on what a mailbox *is*. A tenth key, `folders`, covers every
/// folder somebody made; it has no row of its own here because it is not a
/// mailbox.
pub const MAILBOX_KEYS: [&str; 9] = [
    "inbox", "starred", "snoozed", "sent", "drafts", "outbox", "archive", "spam", "trash",
];

/// The mailbox nobody may hide. Everything else is somebody's business.
pub const ESSENTIAL_MAILBOX: &str = "inbox";

impl Store {
    /// What a mailbox counts when nobody has said otherwise.
    ///
    /// The rule in one function: a list you built by hand counts everything on
    /// it, a place mail lands by itself counts what you have not read, and
    /// Sent counts nothing because nothing waits there.
    pub fn default_count_mode(key: &str) -> CountMode {
        match key {
            "drafts" | "outbox" | "starred" | "snoozed" => CountMode::Total,
            "sent" => CountMode::Off,
            _ => CountMode::Unread,
        }
    }
}

impl CountMode {
    pub fn parse(s: &str) -> CountMode {
        match s {
            "total" => CountMode::Total,
            "off" => CountMode::Off,
            _ => CountMode::Unread,
        }
    }
}

/// Mail that has been thrown away or judged to be junk.
///
/// Every view except Trash and Spam themselves leaves these out, and so does
/// search. Written once because it was written three times: the fourth place
/// that needed it — search — simply did not have it, and quietly returned junk
/// among the results.
fn not_binned(alias: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM placements p
                     JOIN folders f ON f.id = p.folder_id
                     WHERE p.message_id = {alias}.id
                       AND f.role IN ('trash','spam'))"
    )
}

impl ListView {
    /// Rail keys are the wire format, so this is the only place that knows them.
    pub fn parse(key: &str) -> ListView {
        match key {
            "inbox" => ListView::Inbox,
            "starred" => ListView::Starred,
            "archive" | "sent" | "drafts" | "spam" | "trash" => ListView::Folder(key.to_string()),
            "snoozed" => ListView::Snoozed,
            "outbox" => ListView::Outbox,
            "all" => ListView::All,
            other if other.starts_with("folder:") => {
                match other["folder:".len()..].parse::<i64>() {
                    Ok(id) => ListView::UserFolder(id),
                    Err(_) => ListView::Inbox,
                }
            }
            other => match other.strip_prefix("tag:") {
                Some(name) if !name.is_empty() => ListView::Tag(name.to_string()),
                // Anything unrecognised falls back to the inbox rather than
                // erroring: a stale saved view should not leave you looking at
                // a broken screen.
                _ => ListView::Inbox,
            },
        }
    }

    /// The WHERE fragment selecting this view's messages, given the table alias
    /// the surrounding query uses for `messages`.
    ///
    /// The folder role and the tag name are both bound as `?3` rather than
    /// interpolated. Only one of them is ever live, and neither is a literal
    /// this module controls — `Folder` is a public variant and a tag name is
    /// whatever the user typed, so both are user data on its way into SQL.
    fn predicate(&self, alias: &str) -> String {
        match self {
            ListView::Inbox => format!(
                "{} AND ({alias}.snoozed_until_ms IS NULL OR {alias}.snoozed_until_ms <= (strftime('%s','now') * 1000))",
                in_inbox(alias)
            ),
            ListView::Snoozed => {
                format!(
                    "{alias}.snoozed_until_ms IS NOT NULL AND {alias}.snoozed_until_ms > (strftime('%s','now') * 1000)"
                )
            }
            // A draft with a send time is not a draft any more, it is post. The
            // two views are the same rows split on that one column.
            ListView::Outbox => format!("{alias}.send_after_ms IS NOT NULL"),
            ListView::Starred => format!(
                "{alias}.flags & {f} != 0 AND {binned}",
                f = flags::FLAGGED,
                binned = not_binned(alias),
            ),
            ListView::Folder(role) if role == "drafts" => format!(
                "{alias}.send_after_ms IS NULL
                 AND EXISTS (SELECT 1 FROM placements p
                             JOIN folders f ON f.id = p.folder_id
                             WHERE p.message_id = {alias}.id AND f.role = ?3)"
            ),
            // Archive is the one folder that is also a definition. Gmail has
            // no Archive folder — archiving there removes the Inbox label and
            // the message stays in All Mail, which is mapped to this role. So
            // "in the archive folder" is not enough: the day All Mail is
            // synced, every inbox message would have that placement too and
            // Archive would list the entire mailbox. Not-in-the-inbox is what
            // the word actually means, on both kinds of provider.
            ListView::Folder(role) if role == "archive" => format!(
                "EXISTS (SELECT 1 FROM placements p
                         JOIN folders f ON f.id = p.folder_id
                         WHERE p.message_id = {alias}.id
                           AND (f.role = ?3
                                -- A mailbox tree files its history *under*
                                -- Archive: mail in Archive/2023 is archived
                                -- mail, and a view that admitted only the
                                -- bare top folder showed a lifetime of
                                -- filing as empty.
                                OR EXISTS (SELECT 1 FROM folders af
                                           WHERE af.role = 'archive'
                                             AND af.account_id = f.account_id
                                             AND (f.path LIKE af.path || '/%'
                                                  OR f.path LIKE af.path || '.%'))))
                 AND NOT EXISTS (SELECT 1 FROM placements p2
                                 JOIN folders f2 ON f2.id = p2.folder_id
                                 WHERE p2.message_id = {alias}.id AND f2.role = 'inbox')"
            ),
            ListView::Folder(_) => format!(
                "EXISTS (SELECT 1 FROM placements p
                         JOIN folders f ON f.id = p.folder_id
                         WHERE p.message_id = {alias}.id AND f.role = ?3)"
            ),
            // Exactly what is placed there — a user folder is a location,
            // and the list is the location's contents. The id is typed i64,
            // which is what makes writing it into the SQL safe.
            ListView::UserFolder(id) => format!(
                "EXISTS (SELECT 1 FROM placements p
                         WHERE p.message_id = {alias}.id AND p.folder_id = {id})"
            ),
            // No condition at all, which is the point: this is the only view
            // that does not ask where a message sits. The account filter the
            // surrounding query applies is the whole of the selection.
            ListView::All => "1=1".to_string(),
            // Excluding the bins, exactly as Starred does. A tag is a thing
            // you meant; the bin is where things go to stop mattering, and a
            // conversation in it is not still Urgent. Without this, trashing
            // something tagged left it listed under its tag forever.
            ListView::Tag(_) => format!(
                "EXISTS (SELECT 1 FROM message_tags mt
                         JOIN tags tg ON tg.id = mt.tag_id
                         WHERE mt.message_id = {alias}.id AND tg.name = ?3)
                 AND {binned}",
                binned = not_binned(alias),
            ),
        }
    }

    /// The value bound to `?3`, if this view needs one.
    fn bound(&self) -> Option<&str> {
        match self {
            ListView::Folder(role) => Some(role),
            ListView::Tag(name) => Some(name),
            _ => None,
        }
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // Registered before the schema runs: the FTS triggers call these on
        // every write, so they must exist on every connection that opens the
        // store, not only the one that created it.
        let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
        conn.create_scalar_function("petrel_cjk", 1, flags, |ctx| {
            let s: Option<String> = ctx.get(0)?;
            Ok(s.map(|s| cjk_spaced(&s)))
        })?;
        conn.create_scalar_function("petrel_has_cjk", 1, flags, |ctx| {
            let s: Option<String> = ctx.get(0)?;
            Ok(s.is_some_and(|s| has_cjk(&s)))
        })?;

        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Migrations apply in order from whatever version the file is at, so a
        // fresh database runs the baseline and then every step, and an existing
        // one runs only what it is missing. Re-running schema.sql over a
        // populated store would fail on "table already exists".
        let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver < 1 {
            conn.execute_batch(include_str!("schema.sql"))?;
        }
        if ver < 2 {
            conn.execute_batch(include_str!("migrations/0002-tags.sql"))?;
        }
        if ver < 3 {
            conn.execute_batch(include_str!("migrations/0003-settings.sql"))?;
        }
        if ver < 4 {
            conn.execute_batch(include_str!("migrations/0004-action-messages.sql"))?;
        }
        if ver < 5 {
            conn.execute_batch(include_str!("migrations/0005-snooze.sql"))?;
        }
        if ver < 6 {
            conn.execute_batch(include_str!("migrations/0006-identity.sql"))?;
        }
        if ver < 7 {
            conn.execute_batch(include_str!("migrations/0007-draft-body.sql"))?;
        }
        if ver < 8 {
            conn.execute_batch(include_str!("migrations/0008-send-later.sql"))?;
        }
        if ver < 9 {
            conn.execute_batch(include_str!("migrations/0009-remote-senders.sql"))?;
        }
        if ver < 10 {
            conn.execute_batch(include_str!("migrations/0010-draft-html.sql"))?;
        }
        if ver < 11 {
            conn.execute_batch(include_str!("migrations/0011-outbox-state.sql"))?;
        }
        if ver < 12 {
            conn.execute_batch(include_str!("migrations/0012-draft-envelope.sql"))?;
        }
        if ver < 13 {
            conn.execute_batch(include_str!("migrations/0013-draft-sync.sql"))?;
        }
        if ver < 14 {
            conn.execute_batch(include_str!("migrations/0014-rules.sql"))?;
        }
        if ver < 15 {
            conn.execute_batch(include_str!("migrations/0015-thread-key-index.sql"))?;
        }
        if ver < 16 {
            conn.execute_batch(include_str!("migrations/0016-blob-hash-index.sql"))?;
        }
        if ver < 17 {
            conn.execute_batch(include_str!("migrations/0017-gmail-thread-ids.sql"))?;
        }
        if ver < 18 {
            conn.execute_batch(include_str!("migrations/0018-invite-response.sql"))?;
        }
        if ver < 19 {
            conn.execute_batch(include_str!("migrations/0019-trashed-at.sql"))?;
        }
        if ver < 20 {
            conn.execute_batch(include_str!("migrations/0020-sidebar-order.sql"))?;
        }
        if ver < 21 {
            conn.execute_batch(include_str!("migrations/0021-count-view-index.sql"))?;
        }
        if ver < 22 {
            conn.execute_batch(include_str!("migrations/0022-tag-origin.sql"))?;
        }
        if ver < 23 {
            conn.execute_batch(include_str!("migrations/0023-folder-role-index.sql"))?;
        }
        if ver < 24 {
            conn.execute_batch(include_str!("migrations/0024-action-message-outcome.sql"))?;
        }
        if ver < SCHEMA_VERSION {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('extractor_version', ?1)",
                params![EXTRACTOR_VERSION.to_string()],
            )?;
        }
        Ok(Store { conn })
    }

    /// The account the window is showing.
    ///
    /// One at a time, by design: a merged inbox is the single largest source
    /// of send-from-the-wrong-address mistakes, and scoping removes the risk
    /// rather than mitigating it. The choice is remembered in settings, and
    /// falls back to the first account when there is no choice yet or the
    /// chosen one has been removed — so every caller that used to read "the
    /// first account" now reads "the active account" and nothing else changes.
    pub fn active_account(&self) -> Result<Option<i64>> {
        let chosen: Option<i64> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'active_account'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok());
        if let Some(id) = chosen {
            let exists: bool = self
                .conn
                .query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |_| Ok(()))
                .optional()?
                .is_some();
            if exists {
                return Ok(Some(id));
            }
        }
        self.first_account()
    }

    /// Makes an account the one the window shows.
    pub fn set_active_account(&self, account_id: i64) -> Result<()> {
        self.set_setting("active_account", &account_id.to_string())
    }

    /// Every account, oldest first — the order the switcher numbers them in.
    pub fn account_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare("SELECT id FROM accounts ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<i64>, _>>()?;
        Ok(ids)
    }

    /// The first account row, if the store has been used before.
    ///
    /// Only the fallback for `active_account` and the seam onboarding uses to
    /// find the environment-driven row. Everything that shows or acts on mail
    /// wants `active_account`.
    pub fn first_account(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM accounts ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .ok())
    }

    /// Creates an account with its server settings. The password is not
    /// here and never will be: it goes to the OS keychain, keyed by this id.
    pub fn add_account(
        &self,
        kind: &str,
        email: &str,
        display_name: &str,
        servers: &AccountServers,
    ) -> Result<i64> {
        let json = serde_json::to_string(servers).unwrap_or_else(|_| "{}".into());
        let colour = self.next_account_colour()?;
        self.conn.execute(
            "INSERT INTO accounts(kind, email, display_name, settings_json, color)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![kind, email, display_name, json, colour],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Gives an existing account its server settings — the adoption path for
    /// a row that was created from environment variables before onboarding
    /// existed and so has none.
    pub fn set_account_servers(&self, account_id: i64, servers: &AccountServers) -> Result<()> {
        let json = serde_json::to_string(servers).unwrap_or_else(|_| "{}".into());
        self.conn.execute(
            "UPDATE accounts SET settings_json = ?2 WHERE id = ?1",
            params![account_id, json],
        )?;
        Ok(())
    }

    /// An account's server settings, if it has been set up with any. The
    /// row the environment variables created has none, which is how the
    /// caller tells the two apart.
    pub fn account_servers(&self, account_id: i64) -> Result<Option<AccountServers>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT settings_json FROM accounts WHERE id = ?1",
                [account_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str::<AccountServers>(&j).ok()))
    }

    /// Removes an account and everything it holds.
    ///
    /// The foreign keys cascade from `accounts`, so messages, folders, tags
    /// and placements go with the row. Blobs are not touched here: they are
    /// content-addressed and may be shared, and the blob GC reclaims what
    /// nothing references any more.
    pub fn remove_account(&self, account_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM accounts WHERE id = ?1", [account_id])?;
        Ok(())
    }

    pub fn ensure_test_account(&self) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO accounts(kind, email) VALUES ('imap', 'test@example.com')",
            [],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Ingests a real message: raw bytes to blob, parsed view to the database.
    ///
    /// Ordering is deliberate — the blob is written **first**, so a failure
    /// midway leaves at most an unreferenced blob (which GC reclaims), never a
    /// row pointing at bytes that aren't there. The reverse order would mean a
    /// message that exists in the index but cannot be opened.
    ///
    /// Idempotent per (account, Message-ID): re-ingesting the same message —
    /// which happens constantly, since a resync re-fetches — updates rather
    /// than duplicating.
    /// Ingests a message the server holds as a *second copy* of one already
    /// stored — same Message-ID, different UID, both live. Dedupe would fold
    /// it into the first and throw its content away; a draft edited into two
    /// drafts, or a double-delivered message, is two rows on the server and
    /// stays two rows here. The dedupe key is suffixed with the UID so later
    /// refetches of the same copy still land on this row.
    pub fn ingest_raw_second_copy(
        &mut self,
        blobs: &crate::blob::BlobStore,
        account_id: i64,
        folder_id: Option<i64>,
        uid: u32,
        raw: &[u8],
    ) -> Result<Ingested> {
        self.ingest_raw_keyed(blobs, account_id, folder_id, Some(uid), raw, Some(uid))
    }

    pub fn ingest_raw(
        &mut self,
        blobs: &crate::blob::BlobStore,
        account_id: i64,
        folder_id: Option<i64>,
        uid: Option<u32>,
        raw: &[u8],
    ) -> Result<Ingested> {
        self.ingest_raw_keyed(blobs, account_id, folder_id, uid, raw, None)
    }

    fn ingest_raw_keyed(
        &mut self,
        blobs: &crate::blob::BlobStore,
        account_id: i64,
        folder_id: Option<i64>,
        uid: Option<u32>,
        raw: &[u8],
        copy_suffix: Option<u32>,
    ) -> Result<Ingested> {
        let (hash, blob_size) = blobs
            .write(raw)
            .map_err(|e| StoreError::Ingest(format!("blob write failed: {e}")))?;

        let parsed = petrel_mime::parse_message(raw)
            .ok_or_else(|| StoreError::Ingest("unparseable message".into()))?;

        let index_text = parsed.index_text();
        let snippet = preview_of(&index_text);
        let attachment_names = parsed
            .attachments
            .iter()
            .filter_map(|a| a.filename.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let addrs_text = parsed
            .addresses()
            .iter()
            .map(|(_, addr, name)| match name {
                Some(n) => format!("{n} {addr}"),
                None => addr.clone(),
            })
            .collect::<Vec<_>>()
            .join(" ");

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs(hash, kind, size) VALUES (?1, 'raw', ?2)",
            params![hash, blob_size as i64],
        )?;

        // Message-ID is the dedupe key; without one, fall back to the blob hash
        // so a broken message still can't multiply on every resync.
        let mut dedupe_key = parsed
            .message_id
            .clone()
            .unwrap_or_else(|| format!("blake3:{hash}"));
        if let Some(n) = copy_suffix {
            dedupe_key = format!("{dedupe_key}::copy-{n}");
        }

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM messages WHERE account_id = ?1 AND message_id_hdr = ?2",
                params![account_id, dedupe_key],
                |r| r.get(0),
            )
            .ok();

        let date_ms = parsed.date_ms.unwrap_or(0);
        let id = match existing {
            Some(id) => {
                // `deleted_at_ms` cleared: the server just handed this
                // message back, so whatever tombstoned it — a folder pruned
                // from the survey, a deletion seen elsewhere — is no longer
                // true. A folder renamed on another device used to land here
                // with the tombstone intact, invisible in every view and in
                // search until GC purged it thirty days later. The search row
                // is rewritten below with the rest.
                tx.execute(
                    "UPDATE messages SET blob_hash = ?2, blob_kind = 'raw', date_ms = ?3,
                        from_addr = ?4, from_display = ?5, subject = ?6, snippet = ?7,
                        size = ?8, has_attachments = ?9, deleted_at_ms = NULL
                     WHERE id = ?1",
                    params![
                        id,
                        hash,
                        date_ms,
                        parsed.from_addr,
                        parsed.from_display,
                        parsed.subject,
                        snippet,
                        raw.len() as i64,
                        !parsed.attachments.is_empty()
                    ],
                )?;
                tx.execute(
                    "DELETE FROM message_addresses WHERE message_id = ?1",
                    params![id],
                )?;
                tx.execute("DELETE FROM attachments WHERE message_id = ?1", params![id])?;
                tx.execute(
                    "DELETE FROM message_refs WHERE message_id = ?1",
                    params![id],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO messages(account_id, blob_hash, blob_kind, date_ms, from_addr,
                        from_display, subject, snippet, size, message_id_hdr, has_attachments)
                     VALUES (?1, ?2, 'raw', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        account_id,
                        hash,
                        date_ms,
                        parsed.from_addr,
                        parsed.from_display,
                        parsed.subject,
                        snippet,
                        raw.len() as i64,
                        dedupe_key,
                        !parsed.attachments.is_empty()
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };

        {
            let mut ins_addr = tx.prepare_cached(
                "INSERT INTO message_addresses(message_id, role, addr_norm, display)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (role, addr, name) in parsed.addresses() {
                ins_addr.execute(params![id, role, addr, name])?;
            }

            let mut ins_att = tx.prepare_cached(
                "INSERT INTO attachments(message_id, part_id, filename, mime, size)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (i, a) in parsed.attachments.iter().enumerate() {
                ins_att.execute(params![
                    id,
                    i as i64,
                    a.filename,
                    a.content_type,
                    a.size as i64
                ])?;
            }

            if let Some(fid) = folder_id {
                // Not while local triage is still queued. A resync that re-files
                // an archived conversation back into the inbox undoes the user's
                // work on the next launch, long after they did it — which reads
                // as "archiving does not stick" rather than as a sync bug.
                let pending: i64 = tx.query_row(
                    "SELECT EXISTS (
                       SELECT 1 FROM action_messages am
                       JOIN actions a ON a.id = am.action_id
                       WHERE am.message_id = ?1 AND a.state = 'queued'
                     )",
                    params![id],
                    |r| r.get(0),
                )?;
                if pending == 0 {
                    tx.execute(
                        "INSERT OR REPLACE INTO placements(message_id, folder_id, uid) VALUES (?1, ?2, ?3)",
                        params![id, fid, uid],
                    )?;
                }
            }
        }

        let subject_norm =
            crate::threading::normalize_subject(parsed.subject.as_deref().unwrap_or(""));
        assign_thread(
            &tx,
            account_id,
            id,
            parsed.message_id.as_deref(),
            &parsed.references,
            &subject_norm,
            date_ms,
        )?;

        // Same transaction as the message row: the anti-drift invariant.
        tx.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(message_id) DO UPDATE SET
                subject = excluded.subject, body_text = excluded.body_text,
                addrs = excluded.addrs, attachment_names = excluded.attachment_names",
            params![
                id,
                parsed.subject.clone().unwrap_or_default(),
                index_text,
                addrs_text,
                attachment_names
            ],
        )?;
        tx.commit()?;

        Ok(Ingested {
            message_id: id,
            blob_hash: hash,
            was_new: existing.is_none(),
        })
    }

    /// Inserts a batch in one transaction: message row, address rows, and the
    /// searchable text — the anti-drift invariant lives here.
    pub fn insert_messages(&mut self, msgs: &[NewMessage]) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let mut ids = Vec::with_capacity(msgs.len());
        {
            let mut ins_msg = tx.prepare_cached(
                "INSERT INTO messages(account_id, date_ms, from_addr, from_display, subject, snippet)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            let mut ins_addr = tx.prepare_cached(
                "INSERT INTO message_addresses(message_id, role, addr_norm, display)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut ins_fts = tx.prepare_cached(
                "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
                 VALUES (?1, ?2, ?3, ?4, '')",
            )?;
            for m in msgs {
                let snippet: String = m.body_text.chars().take(120).collect();
                ins_msg.execute(params![
                    m.account_id,
                    m.date_ms,
                    m.from_addr,
                    m.from_display,
                    m.subject,
                    snippet
                ])?;
                let id = tx.last_insert_rowid();
                ins_addr.execute(params![
                    id,
                    "from",
                    m.from_addr.to_lowercase(),
                    m.from_display
                ])?;
                ins_addr.execute(params![id, "to", m.to_addr.to_lowercase(), ""])?;
                let addrs = format!("{} {} {}", m.from_addr, m.from_display, m.to_addr);
                ins_fts.execute(params![id, m.subject, m.body_text, addrs])?;
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// How the indexed text is derived. Bumped when that changes.
    ///
    /// The index is built once, when a message arrives, so an improvement to
    /// extraction reaches only new mail — everything already held keeps
    /// whatever the old code produced. This is the version that says otherwise.
    pub const EXTRACTION_VERSION: i64 = 5;

    /// The blob backing a message, for the reading pane to fetch and render.
    /// Who sent a message and when — the two facts an attribution line needs.
    pub fn message_header(&self, message_id: i64) -> Result<Option<(String, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT coalesce(nullif(from_display, ''), coalesce(from_addr, '')), date_ms
                 FROM messages WHERE id = ?1",
                params![message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn blob_hash_for(&self, message_id: i64) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT blob_hash FROM messages WHERE id = ?1")?;
        let hash = stmt
            .query_row(params![message_id], |r| r.get::<_, Option<String>>(0))
            .ok()
            .flatten();
        Ok(hash)
    }

    /// Every stored preference, as a map. Read once at start-up rather than
    /// queried per key: there are a few dozen of them and they are all wanted.
    /// Every account, with the counts and folder mapping the settings pane shows.
    pub fn accounts(&self) -> Result<Vec<AccountSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.id, a.kind, a.email, coalesce(a.display_name,''),
                    coalesce(a.color,''), a.local_archive,
                    (SELECT count(*) FROM messages m
                      WHERE m.account_id = a.id AND m.deleted_at_ms IS NULL),
                    -- The inbox's unread, because that is the number every
                    -- other surface shows. Counting unread *anywhere* had the
                    -- switcher announce seven for an account whose inbox said
                    -- zero — old newsletters filed unread into the archive,
                    -- true but useless, and disagreeing with the pane below.
                    (SELECT count(*) FROM messages m
                      WHERE m.account_id = a.id AND m.deleted_at_ms IS NULL
                        AND m.flags & 1 = 0
                        AND EXISTS (SELECT 1 FROM placements p
                                    JOIN folders f ON f.id = p.folder_id
                                    WHERE p.message_id = m.id AND f.role = 'inbox')
                        AND NOT EXISTS (SELECT 1 FROM placements p
                                        JOIN folders f ON f.id = p.folder_id
                                        WHERE p.message_id = m.id
                                          AND f.role IN ('spam','trash'))),
                    (SELECT max(m.date_ms) FROM messages m
                      WHERE m.account_id = a.id AND m.deleted_at_ms IS NULL)
             FROM accounts a ORDER BY a.id",
        )?;
        let rows: Vec<AccountSummary> = stmt
            .query_map([], |r| {
                Ok(AccountSummary {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    email: r.get(2)?,
                    display_name: r.get(3)?,
                    color: r.get(4)?,
                    local_archive: r.get::<_, i64>(5)? != 0,
                    message_count: r.get(6)?,
                    unread_count: r.get(7)?,
                    newest_ms: r.get(8)?,
                    folders: Vec::new(),
                    active: false,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for mut a in rows {
            let mut f = self.conn.prepare_cached(
                "SELECT coalesce(role,''), path FROM folders
                 WHERE account_id = ?1 AND role IS NOT NULL AND role <> ''
                 ORDER BY role",
            )?;
            a.folders = f
                .query_map(params![a.id], |r| {
                    Ok(FolderMapping {
                        role: r.get(0)?,
                        path: r.get(1)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            out.push(a);
        }
        let active = self.active_account()?;
        for a in &mut out {
            a.active = Some(a.id) == active;
        }
        Ok(out)
    }

    /// The colours an account can wear, in the order they are handed out.
    /// The same six the Settings pane offers, so an assigned colour is always
    /// one the person could have chosen — and can change to any other.
    pub const ACCOUNT_COLOURS: [&'static str; 6] = [
        "#0E7C86", "#9A6B1F", "#6B7F87", "#3B6EA5", "#6B5CA5", "#5E7C4A",
    ];

    /// The first palette colour no other account is wearing.
    ///
    /// Two accounts with the same colour are indistinguishable in the
    /// switcher, which is the one place the colour exists to be read. With
    /// more accounts than colours it wraps, which is the least bad answer.
    pub fn next_account_colour(&self) -> Result<&'static str> {
        let mut stmt = self
            .conn
            .prepare("SELECT coalesce(color,'') FROM accounts")?;
        let used: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self::ACCOUNT_COLOURS
            .iter()
            .find(|c| !used.iter().any(|u| u.eq_ignore_ascii_case(c)))
            .copied()
            .unwrap_or(Self::ACCOUNT_COLOURS[used.len() % Self::ACCOUNT_COLOURS.len()]))
    }

    /// Gives an account a colour if it has none — the rows made before
    /// colours were assigned, and the one the environment makes.
    pub fn ensure_account_colour(&self, account_id: i64) -> Result<()> {
        let has: Option<String> = self
            .conn
            .query_row(
                "SELECT color FROM accounts WHERE id = ?1",
                [account_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if has.as_deref().unwrap_or("").is_empty() {
            let c = self.next_account_colour()?;
            self.set_account_color(account_id, c)?;
        }
        Ok(())
    }

    pub fn set_account_color(&self, account_id: i64, color: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET color = ?2 WHERE id = ?1",
            params![account_id, color],
        )?;
        Ok(())
    }

    /// The account's rules, in run order.
    pub fn rules_for_account(&self, account_id: i64) -> Result<Vec<crate::rules::Rule>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, position, enabled, name, conditions_json, actions_json
             FROM rules WHERE account_id = ?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, position, enabled, name, conds, acts) = row?;
            out.push(crate::rules::Rule {
                id,
                position,
                enabled,
                name,
                conditions: serde_json::from_str(&conds).unwrap_or_default(),
                actions: serde_json::from_str(&acts).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Creates or updates a rule. New rules land at the end of the order.
    pub fn save_rule(
        &mut self,
        account_id: i64,
        rule_id: Option<i64>,
        name: &str,
        enabled: bool,
        conditions: &[crate::rules::Condition],
        actions: &crate::rules::Actions,
    ) -> Result<i64> {
        let conds = serde_json::to_string(conditions).unwrap_or_else(|_| "[]".into());
        let acts = serde_json::to_string(actions).unwrap_or_else(|_| "{}".into());
        match rule_id {
            Some(id) => {
                self.conn.execute(
                    "UPDATE rules SET name = ?2, enabled = ?3, conditions_json = ?4,
                            actions_json = ?5
                     WHERE id = ?1",
                    params![id, name, enabled as i64, conds, acts],
                )?;
                Ok(id)
            }
            None => {
                let position: i64 = self.conn.query_row(
                    "SELECT coalesce(max(position), -1) + 1 FROM rules WHERE account_id = ?1",
                    params![account_id],
                    |r| r.get(0),
                )?;
                self.conn.execute(
                    "INSERT INTO rules(account_id, position, enabled, name, conditions_json, actions_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![account_id, position, enabled as i64, name, conds, acts],
                )?;
                Ok(self.conn.last_insert_rowid())
            }
        }
    }

    pub fn delete_rule(&mut self, rule_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM rules WHERE id = ?1", params![rule_id])?;
        Ok(())
    }

    /// Swaps a rule one step up or down its account's order — the whole of
    /// reordering, because run order is the one thing about a rule that is
    /// not visible in the rule itself.
    pub fn move_rule(&mut self, rule_id: i64, up: bool) -> Result<()> {
        let (account, position): (i64, i64) = self.conn.query_row(
            "SELECT account_id, position FROM rules WHERE id = ?1",
            params![rule_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let neighbour: Option<(i64, i64)> = self
            .conn
            .query_row(
                &format!(
                    "SELECT id, position FROM rules
                     WHERE account_id = ?1 AND position {} ?2
                     ORDER BY position {} LIMIT 1",
                    if up { "<" } else { ">" },
                    if up { "DESC" } else { "ASC" },
                ),
                params![account, position],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((other_id, other_pos)) = neighbour {
            let tx = self.conn.transaction()?;
            tx.execute(
                "UPDATE rules SET position = ?2 WHERE id = ?1",
                params![rule_id, other_pos],
            )?;
            tx.execute(
                "UPDATE rules SET position = ?2 WHERE id = ?1",
                params![other_id, position],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// The identity a message is sent as: who it is from, and what goes at the
    /// bottom.
    pub fn identity(&self, account_id: i64) -> Result<Identity> {
        Ok(self.conn.query_row(
            "SELECT email, coalesce(display_name,''), signature, signature_on_reply
             FROM accounts WHERE id = ?1",
            params![account_id],
            |r| {
                Ok(Identity {
                    address: r.get(0)?,
                    display_name: r.get(1)?,
                    signature: r.get(2)?,
                    signature_on_reply: r.get::<_, i64>(3)? != 0,
                })
            },
        )?)
    }

    pub fn set_identity(
        &self,
        account_id: i64,
        display_name: &str,
        signature: &str,
        signature_on_reply: bool,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts
             SET display_name = ?2, signature = ?3, signature_on_reply = ?4
             WHERE id = ?1",
            params![
                account_id,
                display_name,
                signature,
                if signature_on_reply { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    /// Names the account after the address it actually signs in as.
    ///
    /// The placeholder row exists so the app has something to hang mail off
    /// before any account is configured; once one is, leaving it called
    /// test@example.com tells the user their real mailbox is not connected
    /// while it quietly is. Single-account for now — the account model arrives
    /// with the setup UI, and this becomes an insert rather than an update.
    pub fn set_account_email(&self, account_id: i64, email: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET email = ?2 WHERE id = ?1",
            params![account_id, email],
        )?;
        Ok(())
    }

    /// Records which provider an account turned out to be, once a sync has seen
    /// the server. Called after capability discovery, not guessed at setup.
    pub fn set_account_kind(&self, account_id: i64, kind: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET kind = ?2 WHERE id = ?1",
            params![account_id, kind],
        )?;
        Ok(())
    }

    pub fn settings(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<std::result::Result<std::collections::HashMap<_, _>, _>>()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Removing a preference restores its default, which is not the same as
    /// storing the default: a default that later changes should move with it.
    pub fn clear_setting(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// A tag's name, for the action that has to name it to the server.
    ///
    /// The queued action carries the tag's id, because a name can be edited
    /// between queueing and delivery and the action means "this tag" rather
    /// than "whatever is called that now".
    pub fn tag_name(&self, tag_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT name FROM tags WHERE id = ?1", [tag_id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::match_expr;

    #[test]
    fn match_expr_quotes_and_prefixes() {
        assert_eq!(
            match_expr("hello world", true).unwrap(),
            "\"hello\" \"world\"*"
        );
        assert_eq!(
            match_expr("\"exact phrase\" tail", true).unwrap(),
            "\"exact phrase\" \"tail\"*"
        );
        // FTS5 operators and injection attempts arrive quoted, i.e. inert.
        assert_eq!(match_expr("OR", true).unwrap(), "\"OR\"*");
        // An embedded quote splits tokens; nothing unquoted ever reaches MATCH.
        assert_eq!(match_expr("a\"b", true).unwrap(), "\"a\" \"b\"");
        assert!(match_expr("   ", true).is_none());
    }
}
