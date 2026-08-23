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

pub const SCHEMA_VERSION: i64 = 10;
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
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderMapping {
    /// The SPECIAL-USE role: archive, sent, drafts, spam, trash.
    pub role: String,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Attachment {
    pub filename: String,
    pub size: i64,
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
    /// Display names, for reading — "to Sam Ortiz, Dana Wu".
    pub recipients: Vec<String>,
    /// The same people as addresses, for replying to them. Kept separate
    /// because a reply-all built from display names sends to nobody.
    pub recipient_addrs: Vec<String>,
    pub attachments: Vec<Attachment>,
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
    pub payload_json: String,
    pub message_id: i64,
    /// Absent when the message was never placed in a folder — nothing to
    /// address on the server, so nothing that can be delivered.
    pub uid: Option<u32>,
    pub folder_path: String,
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
    pub subject: String,
    /// Plain text: the snippet, the search index, and the text half of the
    /// message that goes out.
    pub body: String,
    /// The rich-text half, empty for a draft written before there was one.
    pub html: String,
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
    json.and_then(|j| serde_json::from_str::<Vec<ThreadRowTag>>(&j).ok())
        .unwrap_or_default()
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
fn not_filed(alias: &str) -> String {
    // Every role except the inbox itself. Listing only archive/trash/spam let a
    // draft sit in the Drafts folder and in the inbox at the same time, and
    // would have done the same for sent mail the moment a Sent copy was filed.
    // Roles are the closed set the provider mapping produces, so naming them is
    // exact rather than a guess — and a user folder has no role at all, which
    // is why moving something out of the inbox still hides it.
    format!(
        "NOT EXISTS (SELECT 1 FROM placements p
                     JOIN folders f ON f.id = p.folder_id
                     WHERE p.message_id = {alias}.id
                       AND f.role IN ('archive','trash','spam','drafts','sent'))"
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
    /// Put aside until a time that has not arrived yet.
    Snoozed,
    /// Written and waiting to go.
    Outbox,
    Tag(String),
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
                "{} AND coalesce({alias}.snoozed_until_ms, 0) <= (strftime('%s','now') * 1000)",
                not_filed(alias)
            ),
            ListView::Snoozed => {
                format!("coalesce({alias}.snoozed_until_ms, 0) > (strftime('%s','now') * 1000)")
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
                         WHERE p.message_id = {alias}.id AND f.role = ?3)
                 AND NOT EXISTS (SELECT 1 FROM placements p2
                                 JOIN folders f2 ON f2.id = p2.folder_id
                                 WHERE p2.message_id = {alias}.id AND f2.role = 'inbox')"
            ),
            ListView::Folder(_) => format!(
                "EXISTS (SELECT 1 FROM placements p
                         JOIN folders f ON f.id = p.folder_id
                         WHERE p.message_id = {alias}.id AND f.role = ?3)"
            ),
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
        if ver < SCHEMA_VERSION {
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('extractor_version', ?1)",
                params![EXTRACTOR_VERSION.to_string()],
            )?;
        }
        Ok(Store { conn })
    }

    /// The first account row, if the store has been used before.
    pub fn first_account(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM accounts ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .ok())
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
    pub fn ingest_raw(
        &mut self,
        blobs: &crate::blob::BlobStore,
        account_id: i64,
        folder_id: Option<i64>,
        uid: Option<u32>,
        raw: &[u8],
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
        let dedupe_key = parsed
            .message_id
            .clone()
            .unwrap_or_else(|| format!("blake3:{hash}"));

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
                tx.execute(
                    "UPDATE messages SET blob_hash = ?2, blob_kind = 'raw', date_ms = ?3,
                        from_addr = ?4, from_display = ?5, subject = ?6, snippet = ?7,
                        size = ?8, has_attachments = ?9
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

    pub fn delete_message(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
        tx.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_body(&mut self, id: i64, new_body: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE fts_content SET body_text = ?2 WHERE message_id = ?1",
            params![id, new_body],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Routed search: CJK queries use the per-character index, everything else
    /// the unicode61 index with as-you-type prefix on the final token.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        if query.chars().any(is_cjk) {
            self.search_cjk(query, limit)
        } else {
            self.search_unicode(query, limit)
        }
    }

    /// Matches are marked with U+E000 and U+E001, not square brackets.
    ///
    /// Brackets are ordinary text in mail. The plain-text alternative that
    /// marketing senders generate is full of things like [image: Google], and
    /// with brackets as the marker the renderer highlighted the sender's own
    /// punctuation as though it had matched the search. Nothing types a
    /// private-use codepoint, so nothing can be mistaken for one.
    pub fn search_unicode(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        let Some(expr) = match_expr(query, true) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT rowid,
                    bm25(fts_messages, 4.0, 1.0, 2.0, 2.0) AS r,
                    snippet(fts_messages, 1, char(57344), char(57345), '…', 12)
             FROM fts_messages
             WHERE fts_messages MATCH ?1
             ORDER BY r
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![expr, limit], |row| {
            Ok(SearchHit {
                message_id: row.get(0)?,
                rank: row.get(1)?,
                snippet: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Per-character CJK search. Ranks on the index copy but takes snippets from
    /// `fts_content`, because the indexed text is space-separated.
    pub fn search_cjk(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        let Some(expr) = cjk_match_expr(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.rowid,
                    bm25(fts_cjk, 4.0, 1.0) AS r,
                    c.body_text
             FROM fts_cjk f
             JOIN fts_content c ON c.message_id = f.rowid
             WHERE fts_cjk MATCH ?1
             ORDER BY r
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![expr, limit], |row| {
            let body: String = row.get(2)?;
            Ok(SearchHit {
                message_id: row.get(0)?,
                rank: row.get(1)?,
                snippet: cjk_snippet(&body, query),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// How many messages carry a CJK index entry. Zero for a mailbox with no
    /// CJK at all — the property that keeps this index from costing everyone.
    pub fn cjk_indexed_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM fts_cjk", [], |r| r.get(0))?)
    }

    /// How the indexed text is derived. Bumped when that changes.
    ///
    /// The index is built once, when a message arrives, so an improvement to
    /// extraction reaches only new mail — everything already held keeps
    /// whatever the old code produced. This is the version that says otherwise.
    pub const EXTRACTION_VERSION: i64 = 3;

    /// Re-derives the indexed text of every stored message from its bytes.
    ///
    /// Runs when the extraction version moves, and not otherwise: it re-parses
    /// every blob, which is cheap for a few hundred messages and not something
    /// to do at every launch. Returns how many were rewritten.
    pub fn reindex_bodies(&mut self, blobs: &crate::blob::BlobStore) -> Result<usize> {
        let held: i64 = self
            .settings()?
            .get("extraction_version")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if held >= Self::EXTRACTION_VERSION {
            return Ok(0);
        }

        let rows: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, blob_hash FROM messages
                 WHERE blob_hash IS NOT NULL AND deleted_at_ms IS NULL",
            )?;
            let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let tx = self.conn.transaction()?;
        let mut done = 0usize;
        {
            // Both the index and the row preview. The first pass rewrote only
            // the index, which is why placeholders kept showing on rows: the
            // preview is a separate column, written once at ingest, and no
            // amount of reindexing touches it.
            let mut update =
                tx.prepare("UPDATE fts_content SET body_text = ?2 WHERE message_id = ?1")?;
            let mut preview = tx.prepare("UPDATE messages SET snippet = ?2 WHERE id = ?1")?;
            for (id, hash) in rows {
                // A blob that will not read is not a reason to abandon the
                // rest; it keeps whatever text it already had.
                let Ok(raw) = blobs.read(&hash) else { continue };
                let Some(parsed) = petrel_mime::parse_message(&raw) else {
                    continue;
                };
                let text = parsed.index_text();
                update.execute(params![id, &text])?;
                preview.execute(params![id, preview_of(&text)])?;
                done += 1;
            }
        }
        tx.commit()?;
        self.rebuild_fts()?;
        self.set_setting("extraction_version", &Self::EXTRACTION_VERSION.to_string())?;
        Ok(done)
    }

    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('rebuild')",
            [],
        )?;
        // Not external-content, so FTS5's 'rebuild' cannot reach it: repopulate
        // from fts_content, which is the source of truth either way.
        self.conn.execute("DELETE FROM fts_cjk", [])?;
        self.conn.execute(
            "INSERT INTO fts_cjk(rowid, subject, body_text)
             SELECT message_id, petrel_cjk(subject), petrel_cjk(body_text)
             FROM fts_content
             WHERE petrel_has_cjk(subject) OR petrel_has_cjk(body_text)",
            [],
        )?;
        Ok(())
    }

    pub fn optimize_fts(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('optimize')",
            [],
        )?;
        self.conn
            .execute("INSERT INTO fts_cjk(fts_cjk) VALUES('optimize')", [])?;
        Ok(())
    }

    /// Verifies each FTS index against `fts_content`; errors on divergence.
    pub fn fts_integrity_check(&self) -> Result<()> {
        for t in ["fts_messages", "fts_cjk"] {
            let sql = format!("INSERT INTO {t}({t}) VALUES('integrity-check')");
            if let Err(e) = self.conn.execute(&sql, []) {
                return Err(StoreError::Integrity(format!("{t}: {e}")));
            }
        }
        Ok(())
    }

    /// Sets an account's retention mode (Q24).
    pub fn set_local_archive(&self, account_id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET local_archive = ?2 WHERE id = ?1",
            params![account_id, enabled],
        )?;
        Ok(())
    }

    pub fn retention_mode(&self, account_id: i64) -> Result<RetentionMode> {
        let flag: bool = self.conn.query_row(
            "SELECT local_archive FROM accounts WHERE id = ?1",
            params![account_id],
            |r| r.get(0),
        )?;
        Ok(RetentionMode::from_flag(flag))
    }

    /// Applies a server's truth to local state: any message we hold for this
    /// account that the server no longer lists is soft-deleted.
    ///
    /// Soft delete removes it from search and lists immediately (the user asked
    /// for it gone) while keeping the row and blob recoverable until GC. In
    /// `LocalArchive` mode nothing is removed at all — that is the entire point
    /// of the mode, so this returns 0 without touching anything.
    ///
    /// `present_message_ids` are the dedupe keys still on the server.
    pub fn reconcile_server_absences(
        &mut self,
        account_id: i64,
        present_message_ids: &[String],
        now_ms: i64,
    ) -> Result<usize> {
        if self.retention_mode(account_id)? == RetentionMode::LocalArchive {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        let present: std::collections::HashSet<&str> =
            present_message_ids.iter().map(|s| s.as_str()).collect();

        let candidates: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, coalesce(message_id_hdr, '') FROM messages
                 WHERE account_id = ?1 AND deleted_at_ms IS NULL",
            )?;
            let rows = stmt.query_map(params![account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut removed = 0;
        for (id, key) in candidates {
            if present.contains(key.as_str()) {
                continue;
            }
            tx.execute(
                "UPDATE messages SET deleted_at_ms = ?2 WHERE id = ?1",
                params![id, now_ms],
            )?;
            // Out of the index immediately: a deleted message must not surface
            // in search while it waits out the grace period.
            tx.execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
            removed += 1;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Restores a soft-deleted message and re-indexes it from its stored bytes.
    /// Possible only while the blob survives — i.e. within the grace period.
    pub fn restore_message(
        &mut self,
        blobs: &crate::blob::BlobStore,
        message_id: i64,
    ) -> Result<bool> {
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT blob_hash FROM messages WHERE id = ?1 AND deleted_at_ms IS NOT NULL",
                params![message_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let Some(hash) = hash else {
            return Ok(false);
        };
        let Ok(raw) = blobs.read(&hash) else {
            return Ok(false);
        };
        let Some(parsed) = petrel_mime::parse_message(&raw) else {
            return Ok(false);
        };

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE messages SET deleted_at_ms = NULL WHERE id = ?1",
            params![message_id],
        )?;
        tx.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(message_id) DO UPDATE SET
                subject = excluded.subject, body_text = excluded.body_text",
            params![
                message_id,
                parsed.subject.clone().unwrap_or_default(),
                parsed.index_text(),
                parsed
                    .addresses()
                    .iter()
                    .map(|(_, a, _)| a.clone())
                    .collect::<Vec<_>>()
                    .join(" "),
                parsed
                    .attachments
                    .iter()
                    .filter_map(|a| a.filename.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Destroys soft-deleted mail whose grace period has expired, then reclaims
    /// any blob no message references any more.
    ///
    /// Blob reclamation is reachability-based rather than per-message, because
    /// blobs are shared by content hash — deleting one message's file could
    /// otherwise blank an identical message in another account.
    pub fn gc(
        &mut self,
        blobs: &crate::blob::BlobStore,
        now_ms: i64,
        grace_days: i64,
    ) -> Result<GcReport> {
        let cutoff = now_ms.saturating_sub(grace_days.saturating_mul(crate::retention::MS_PER_DAY));

        let tx = self.conn.transaction()?;
        let purged = tx.execute(
            "DELETE FROM messages WHERE deleted_at_ms IS NOT NULL AND deleted_at_ms <= ?1",
            params![cutoff],
        )?;

        // Orphans: registered blobs no live row points at.
        let orphans: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT b.hash FROM blobs b
                 WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.blob_hash = b.hash)
                   AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.blob_hash = b.hash)",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for hash in &orphans {
            tx.execute("DELETE FROM blobs WHERE hash = ?1", params![hash])?;
        }
        tx.commit()?;

        // Files last: a crash here leaves a registered-but-absent blob, which
        // reads as corruption and heals by refetch. The reverse order would
        // delete bytes a live row still points at.
        let mut blobs_removed = 0;
        for hash in &orphans {
            if blobs.remove(hash).is_ok() {
                blobs_removed += 1;
            }
        }

        Ok(GcReport {
            messages_purged: purged,
            blobs_removed,
        })
    }

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
                    -- Unread that a person would claim as theirs. Spam and
                    -- the bin are excluded: now that both are synced, counting
                    -- them would have the account header announce unread mail
                    -- whose whole point is that it was already dealt with.
                    (SELECT count(*) FROM messages m
                      WHERE m.account_id = a.id AND m.deleted_at_ms IS NULL
                        AND m.flags & 1 = 0
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
        Ok(out)
    }

    pub fn set_account_color(&self, account_id: i64, color: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET color = ?2 WHERE id = ?1",
            params![account_id, color],
        )?;
        Ok(())
    }

    /// The folder holding a role for this account, if one is mapped.
    pub fn folder_for_role(&self, account_id: i64, role: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND role = ?2 LIMIT 1",
                params![account_id, role],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Creates a folder for a role if the account has none. Real accounts learn
    /// their folders from the server; this exists for accounts that have not
    /// synced and for tests, so triage has somewhere to move mail to.
    pub fn ensure_folder(&self, account_id: i64, role: &str, path: &str) -> Result<i64> {
        if let Some(id) = self.folder_for_role(account_id, role)? {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO folders(account_id, role, name, path) VALUES (?1, ?2, ?3, ?4)",
            params![account_id, role, path, path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Every folder on the account, for the move picker.
    ///
    /// Folders the user made come first, because those are what a move is
    /// usually for; the role folders are reachable from the rail already and
    /// are here only so V can get to them too.
    pub fn folders(&self, account_id: i64) -> Result<Vec<FolderSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(role,''), path FROM folders
             WHERE account_id = ?1
             ORDER BY (role IS NOT NULL AND role <> ''), path COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok(FolderSummary {
                id: r.get(0)?,
                role: r.get(1)?,
                path: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Records the folders a server reported, with the roles it claimed.
    ///
    /// Upsert by path rather than insert: a resync must not duplicate every
    /// folder, and a server that starts advertising SPECIAL-USE later should
    /// update the role on the row that already exists rather than shadow it.
    pub fn sync_folders(
        &self,
        account_id: i64,
        folders: &[(String, Option<String>)],
    ) -> Result<usize> {
        let mut n = 0;
        for (path, role) in folders {
            let name = path.rsplit('/').next().unwrap_or(path);
            let existing: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM folders WHERE account_id = ?1 AND path = ?2",
                    params![account_id, path],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(id) => {
                    self.conn.execute(
                        "UPDATE folders SET role = ?2, name = ?3 WHERE id = ?1",
                        params![id, role, name],
                    )?;
                }
                None => {
                    self.conn.execute(
                        "INSERT INTO folders(account_id, role, name, path) VALUES (?1, ?2, ?3, ?4)",
                        params![account_id, role, name, path],
                    )?;
                }
            }
            n += 1;
        }
        Ok(n)
    }

    /// A folder the user named, looked up by path and created if new.
    ///
    /// Separate from `ensure_folder`, which keys on role: two user folders can
    /// exist side by side with no role at all, so role is the wrong identity
    /// for them.
    pub fn ensure_named_folder(&self, account_id: i64, path: &str) -> Result<i64> {
        let path = path.trim();
        if path.is_empty() {
            return Err(StoreError::Rejected("a folder needs a name".into()));
        }
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND path = ?2 COLLATE NOCASE",
                params![account_id, path],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        // The leaf is the display name; the path keeps the hierarchy the server
        // uses, so "Contracts/2026" shows as 2026 nested under Contracts.
        let name = path.rsplit('/').next().unwrap_or(path);
        self.conn.execute(
            "INSERT INTO folders(account_id, role, name, path) VALUES (?1, NULL, ?2, ?3)",
            params![account_id, name, path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn place_message(&self, message_id: i64, folder_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO placements(message_id, folder_id) VALUES (?1, ?2)",
            params![message_id, folder_id],
        )?;
        Ok(())
    }

    pub fn folders_of(&self, message_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT folder_id FROM placements WHERE message_id = ?1")?;
        let rows = stmt.query_map(params![message_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// How this account's provider models placement.
    ///
    /// Derived from the account kind rather than sniffed per action, so every
    /// caller gets the same answer and a mis-detected provider is one row to
    /// fix rather than a behaviour scattered across call sites. Unknown
    /// providers get the exclusive model: it is the IMAP default, and guessing
    /// "labels" for a server that does not have them would leave messages in
    /// two folders that can only hold them in one.
    pub fn placement_policy(&self, account_id: i64) -> Result<crate::actions::PlacementPolicy> {
        use crate::actions::PlacementPolicy;
        let kind: String = self
            .conn
            .query_row(
                "SELECT kind FROM accounts WHERE id = ?1",
                params![account_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        Ok(match kind.as_str() {
            "gmail" => PlacementPolicy::Labels,
            _ => PlacementPolicy::Exclusive,
        })
    }

    /// The highest UID stored for a folder, or None when it holds nothing yet.
    ///
    /// This is what makes a poll cheap: everything above it is new, so a resync
    /// asks for `{max+1}:*` rather than refetching the last N messages by
    /// sequence number every time — which is both wasteful and wrong, since
    /// sequence numbers shift as mail arrives and is expunged.
    pub fn max_uid(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT max(uid) FROM placements WHERE folder_id = ?1",
                params![folder_id],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .map(|u| u as u32))
    }

    /// The server path of a folder, for addressing it over IMAP.
    pub fn folder_path(&self, folder_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The queued actions, oldest first, with the UID and folder each one needs
    /// to reach the server.
    ///
    /// Oldest first matters: two actions on the same message must arrive in the
    /// order the user performed them, or the later one loses.
    pub fn pending_actions(&self, account_id: i64) -> Result<Vec<PendingAction>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.kind, a.payload_json, am.message_id, p.uid, f.path
             FROM actions a
             JOIN action_messages am ON am.action_id = a.id
             LEFT JOIN placements p ON p.message_id = am.message_id
             LEFT JOIN folders f ON f.id = p.folder_id
             WHERE a.account_id = ?1 AND a.state = 'queued'
             ORDER BY a.id, am.message_id",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok(PendingAction {
                action_id: r.get(0)?,
                kind_json: r.get(1)?,
                payload_json: r.get(2)?,
                message_id: r.get(3)?,
                uid: r.get::<_, Option<i64>>(4)?.map(|u| u as u32),
                folder_path: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Whether this message has local changes the server has not been told about.
    ///
    /// A resync must not overwrite those. The server is authoritative about a
    /// message only once our queued actions against it have been delivered —
    /// until then, applying the server's version silently undoes what the user
    /// just did, and does it on the next launch rather than immediately, which
    /// is the hardest possible version of that bug to recognise.
    pub fn message_has_pending(&self, message_id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM action_messages am
               JOIN actions a ON a.id = am.action_id
               WHERE am.message_id = ?1 AND a.state = 'queued'
             )",
            params![message_id],
            |r| r.get::<_, i64>(0),
        )? == 1)
    }

    /// Assigns the IMAP system flags a server reported, replacing whatever was
    /// there.
    ///
    /// Assignment rather than add/remove because this is the server stating
    /// what is true, not a user action nudging it: a message that was read
    /// elsewhere and has since been marked unread has to end up unread here,
    /// and an add-only merge could never take a flag away.
    pub fn set_message_flags(&self, message_id: i64, flags: i64) -> Result<()> {
        // Local pending work wins. See message_has_pending.
        if self.message_has_pending(message_id)? {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE messages SET flags = ?2 WHERE id = ?1",
            params![message_id, flags],
        )?;
        Ok(())
    }

    /// Saves a draft, or updates one already saved.
    ///
    /// Stored as an ordinary message row carrying the \Draft flag and placed in
    /// the drafts folder, rather than in a table of its own. That is what makes
    /// the Drafts view, search, and every triage action work on drafts without
    /// any of them learning a second kind of thing — and it is how a draft
    /// reaches the server the day sync learns to APPEND one.
    pub fn save_draft(
        &self,
        account_id: i64,
        draft_id: Option<i64>,
        to: &str,
        subject: &str,
        body: &str,
        html: &str,
    ) -> Result<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // The list shows a snippet; an empty draft still needs to be findable,
        // so it gets a placeholder rather than a blank row.
        let snippet: String = body.chars().take(200).collect();
        let identity = self.identity(account_id)?;

        let id = match draft_id {
            Some(id) => {
                self.conn.execute(
                    "UPDATE messages
                     SET date_ms = ?2, subject = ?3, snippet = ?4, draft_body = ?5,
                         draft_html = ?6
                     WHERE id = ?1",
                    params![id, now, subject, snippet, body, html],
                )?;
                id
            }
            None => {
                self.conn.execute(
                    "INSERT INTO messages(account_id, date_ms, from_addr, from_display,
                                          subject, snippet, draft_body, draft_html, flags)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        account_id,
                        now,
                        identity.address,
                        identity.display_name,
                        subject,
                        snippet,
                        body,
                        html,
                        flags::DRAFT | flags::SEEN
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
        };

        // Recipients live where every other message keeps them, so the list can
        // show who a draft is to without a special case.
        self.conn.execute(
            "DELETE FROM message_addresses WHERE message_id = ?1",
            params![id],
        )?;
        for addr in to
            .split([',', ';'])
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            self.conn.execute(
                "INSERT INTO message_addresses(message_id, role, addr_norm, display)
                 VALUES (?1, 'to', ?2, ?2)",
                params![id, addr],
            )?;
        }

        let folder = self.ensure_folder(account_id, "drafts", "drafts")?;
        self.conn
            .execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
        self.place_message(id, folder)?;
        Ok(id)
    }

    /// Marks a draft to go at a given time, or clears the schedule.
    ///
    /// Clearing matters as much as setting: an outbox you cannot pull something
    /// back out of is a worse promise than sending straight away, because the
    /// window where you can change your mind is exactly why it exists.
    pub fn schedule_send(&self, draft_id: i64, at_ms: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET send_after_ms = ?2 WHERE id = ?1",
            params![draft_id, at_ms],
        )?;
        Ok(())
    }

    /// Drafts whose time has come.
    ///
    /// A comparison against the clock, not a timer — so a message due while the
    /// app was closed goes out on the next pass instead of being missed by an
    /// alarm that never rang.
    pub fn due_sends(&self, account_id: i64, now_ms: i64) -> Result<Vec<DraftRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM messages
             WHERE account_id = ?1 AND send_after_ms IS NOT NULL AND send_after_ms <= ?2
             ORDER BY send_after_ms",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(params![account_id, now_ms], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter().map(|id| self.load_draft(id)).collect()
    }

    /// Reads a draft back for editing.
    pub fn load_draft(&self, id: i64) -> Result<DraftRecord> {
        let (subject, body, html): (String, String, String) = self.conn.query_row(
            "SELECT coalesce(subject,''), coalesce(draft_body,''), coalesce(draft_html,'')
             FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT addr_norm FROM message_addresses WHERE message_id = ?1 AND role = 'to'",
        )?;
        let to: Vec<String> = stmt
            .query_map(params![id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(DraftRecord {
            id,
            to: to.join(", "),
            subject,
            body,
            html,
        })
    }

    /// Removes a draft once it has been sent or discarded.
    pub fn delete_draft(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM messages WHERE id = ?1", params![id])?;
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

    pub fn tags_of(&self, message_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT tag_id FROM message_tags WHERE message_id = ?1")?;
        let rows = stmt.query_map(params![message_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Applies a triage action to a whole conversation, locally and at once,
    /// then queues it for the server. Returns a receipt that carries everything
    /// undo needs, so callers hold no state of their own.
    pub fn apply_thread_action(
        &self,
        account_id: i64,
        thread_id: i64,
        kind: crate::actions::ActionKind,
        target: Option<i64>,
        policy: crate::actions::PlacementPolicy,
    ) -> Result<crate::actions::ActionReceipt> {
        use crate::actions::{ActionKind, ActionPayload, ActionReceipt, PriorState};

        // Refused here rather than defaulted to something plausible: a move with
        // no destination is a bug in the caller, and inventing one would file
        // mail somewhere nobody asked for.
        if kind.needs_target() && target.is_none() {
            return Err(StoreError::Rejected(format!(
                "{kind:?} needs a target folder or tag"
            )));
        }

        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id FROM messages
                 WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL",
            )?;
            stmt.query_map(params![thread_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        // Captured *before* anything changes: undo restores what was, rather
        // than guessing an inverse — which breaks as soon as two actions touch
        // the same message.
        let mut prior = Vec::with_capacity(ids.len());
        for id in &ids {
            let flags: i64 = self.conn.query_row(
                "SELECT flags FROM messages WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            prior.push(PriorState {
                message_id: *id,
                flags,
                tag_ids: self.tags_of(*id)?,
                snoozed_until: self.conn.query_row(
                    "SELECT snoozed_until_ms FROM messages WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )?,
                folder_ids: self.folders_of(*id)?,
            });
        }

        for id in &ids {
            match kind {
                ActionKind::Star => self.set_flags(*id, flags::FLAGGED, 0)?,
                ActionKind::Unstar => self.set_flags(*id, 0, flags::FLAGGED)?,
                ActionKind::MarkRead => self.set_flags(*id, flags::SEEN, 0)?,
                ActionKind::MarkUnread => self.set_flags(*id, 0, flags::SEEN)?,
                ActionKind::Snooze => {
                    self.conn.execute(
                        "UPDATE messages SET snoozed_until_ms = ?2 WHERE id = ?1",
                        params![id, target.expect("checked above")],
                    )?;
                }
                ActionKind::Unsnooze => {
                    self.conn.execute(
                        "UPDATE messages SET snoozed_until_ms = NULL WHERE id = ?1",
                        params![id],
                    )?;
                }
                ActionKind::DeleteForever => {
                    // A tombstone rather than a DELETE, and deliberately so:
                    // the queued action still refers to this message id, and
                    // removing the row before the server has been told would
                    // strand the instruction that makes the deletion real.
                    // The row and its bytes are reaped later, by the same
                    // grace-period sweep that handles mail the server dropped.
                    // The clock comes from SQLite rather than a parameter,
                    // as the snooze predicate's does: this timestamp only ever
                    // feeds the grace-period sweep, and threading a clock
                    // through triage to stamp a tombstone is not worth it.
                    self.conn.execute(
                        "UPDATE messages SET deleted_at_ms = (strftime('%s','now') * 1000)
                         WHERE id = ?1",
                        params![id],
                    )?;
                    // Out of search at once. A message the user deleted must
                    // not keep answering queries while its bytes are reaped.
                    self.conn
                        .execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
                }
                ActionKind::Tag => self.tag_message(*id, target.expect("checked above"))?,
                ActionKind::Untag => self.untag_message(*id, target.expect("checked above"))?,
                ActionKind::Move => {
                    let dest = target.expect("checked above");
                    self.conn
                        .execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
                    self.place_message(*id, dest)?;
                }
                ActionKind::Archive => {
                    let dest = self.ensure_folder(account_id, "archive", "archive")?;
                    if policy.archive_clears_everything() {
                        self.conn
                            .execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
                    } else {
                        // Labels: archiving removes the message from the inbox
                        // and leaves every other label alone. Clearing them all
                        // would throw away folders the user filed it under
                        // deliberately — invisibly, and before the server has
                        // even been asked.
                        self.conn.execute(
                            "DELETE FROM placements
                             WHERE message_id = ?1
                               AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
                            params![id],
                        )?;
                    }
                    self.place_message(*id, dest)?;
                }
                ActionKind::Trash | ActionKind::Spam => {
                    let role = kind.destination_role().expect("move action has a role");
                    let dest = self.ensure_folder(account_id, role, role)?;
                    // Binning is exclusive on both providers: a message in the
                    // trash is not still sitting in your inbox under a label.
                    self.conn
                        .execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
                    self.place_message(*id, dest)?;
                }
            }
        }

        let payload = ActionPayload {
            kind,
            thread_id,
            target,
            prior,
        };
        let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
        self.conn.execute(
            "INSERT INTO actions(account_id, kind, payload_json, state, created_ms)
             VALUES (?1, ?2, ?3, ?5, ?4)",
            params![
                account_id,
                serde_json::to_string(&kind).unwrap_or_default(),
                json,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                if kind.is_local_only() {
                    "local"
                } else {
                    "queued"
                }
            ],
        )?;

        let action_id = self.conn.last_insert_rowid();
        for id in &ids {
            self.conn.execute(
                "INSERT OR IGNORE INTO action_messages(action_id, message_id) VALUES (?1, ?2)",
                params![action_id, id],
            )?;
        }

        Ok(ActionReceipt {
            action_id,
            kind,
            message_count: ids.len(),
            description: kind.past_tense().to_string(),
        })
    }

    /// Puts back exactly what an action replaced. Only works while the action is
    /// still queued: once it has reached the server, undoing is a new action
    /// rather than a cancellation, and pretending otherwise would be a lie about
    /// what the other end knows.
    pub fn undo_action(&self, action_id: i64) -> Result<bool> {
        use crate::actions::ActionPayload;

        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT payload_json, state FROM actions WHERE id = ?1",
                params![action_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((json, state)) = row else {
            return Ok(false);
        };
        // 'local' actions never go to a server, so there is no point at which
        // undoing one stops being a cancellation.
        if state != "queued" && state != "local" {
            return Ok(false);
        }
        let Ok(payload) = serde_json::from_str::<ActionPayload>(&json) else {
            return Ok(false);
        };
        // Belt as well as braces. The UI confirms a permanent delete instead of
        // offering undo, but this is the layer that must not be talked into
        // restoring placements for a message whose bytes are being expunged —
        // that would resurrect a row pointing at mail nobody can fetch again.
        if !payload.kind.is_undoable() {
            return Ok(false);
        }

        for p in &payload.prior {
            self.conn.execute(
                "UPDATE messages SET flags = ?2 WHERE id = ?1",
                params![p.message_id, p.flags],
            )?;
            self.conn.execute(
                "DELETE FROM placements WHERE message_id = ?1",
                params![p.message_id],
            )?;
            for f in &p.folder_ids {
                self.place_message(p.message_id, *f)?;
            }
            // Same shape as folders: wipe and restore what was captured, rather
            // than removing whatever this action added. Those differ whenever
            // the tag was already on the message before the action ran.
            self.conn.execute(
                "DELETE FROM message_tags WHERE message_id = ?1",
                params![p.message_id],
            )?;
            for tag in &p.tag_ids {
                self.tag_message(p.message_id, *tag)?;
            }
            self.conn.execute(
                "UPDATE messages SET snoozed_until_ms = ?2 WHERE id = ?1",
                params![p.message_id, p.snoozed_until],
            )?;
        }

        self.conn.execute(
            "UPDATE actions SET state = 'undone' WHERE id = ?1",
            params![action_id],
        )?;
        Ok(true)
    }

    pub fn flags_of(&self, message_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT flags FROM messages WHERE id = ?1",
            params![message_id],
            |r| r.get(0),
        )?)
    }

    /// Moves a queued action along. The dispatcher owns this; it is here so
    /// tests can reach the state where undo must refuse.
    pub fn mark_action_state(&self, action_id: i64, state: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE actions SET state = ?2 WHERE id = ?1",
            params![action_id, state],
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

    /// Ids of the most recent messages — used by maintenance and demo paths that
    /// need to walk the store without materialising whole rows.
    pub fn recent_ids(&self, limit: u32) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM messages WHERE deleted_at_ms IS NULL
             ORDER BY date_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// True when every message came from the synthetic demo generators. Used
    /// to decide whether a store may be wiped for a re-seed; a single real
    /// message makes this false.
    pub fn all_messages_synthetic(&self) -> Result<bool> {
        let foreign: i64 = self.conn.query_row(
            "SELECT count(*) FROM messages
             WHERE from_addr NOT LIKE '%example.com'
               AND from_addr NOT LIKE '%example.org'
               AND from_addr NOT LIKE '%example.net'
               AND from_addr NOT LIKE '%example.io'
               AND from_addr NOT LIKE '%example.dev'
               AND from_addr NOT LIKE '%.example'
               AND from_addr NOT LIKE '%example.jp'",
            [],
            |r| r.get(0),
        )?;
        Ok(foreign == 0)
    }

    /// Demo-path only: empties the mailbox. Callers must have established that
    /// the store holds nothing real.
    pub fn delete_all_messages(&self) -> Result<usize> {
        let n = self.conn.execute("DELETE FROM messages", [])?;
        self.conn.execute("DELETE FROM fts_content", [])?;
        self.conn.execute("DELETE FROM fts_cjk", [])?;
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('rebuild')",
            [],
        )?;
        Ok(n)
    }

    /// Writes a view's messages to an mbox file, oldest first.
    ///
    /// mbox because it is the format every other mail client can read — the
    /// promise being kept here is that your mail is yours whatever happens to
    /// this program, and a proprietary export would break that promise while
    /// appearing to honour it.
    ///
    /// Returns how many messages were written. Messages whose blob is missing
    /// are skipped and counted separately rather than aborting the export: a
    /// partial archive of 9,000 messages is worth more than an error and none.
    pub fn export_mbox(
        &self,
        blobs: &crate::blob::BlobStore,
        view: &ListView,
        path: &Path,
    ) -> Result<(usize, usize)> {
        use std::io::Write;

        let ids: Vec<(i64, String, String, i64)> = {
            // Every message in the view's conversations, not just the newest of
            // each: an archive that keeps one message per thread is not an
            // archive of your mail.
            let threads: Vec<i64> = self
                .list_threads(view, 0, u32::MAX)?
                .into_iter()
                .map(|t| t.thread_id)
                .collect();
            let mut out = Vec::new();
            for tid in threads {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT id, coalesce(blob_hash,''), coalesce(from_addr,''), date_ms
                     FROM messages
                     WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL
                     ORDER BY date_ms",
                )?;
                let rows = stmt.query_map(params![tid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
                for row in rows {
                    out.push(row?);
                }
            }
            out.sort_by_key(|(_, _, _, date)| *date);
            out
        };

        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut written = 0usize;
        let mut skipped = 0usize;

        for (_, hash, from, date_ms) in ids {
            let Ok(raw) = blobs.read(&hash) else {
                skipped += 1;
                continue;
            };
            // The "From " line mbox separates on, in the asctime shape readers
            // expect. Anything unparseable still gets a line, because a reader
            // that cannot find the separator sees one enormous message.
            let stamp = format_asctime(date_ms);
            let sender = if from.is_empty() {
                "petrel@localhost"
            } else {
                &from
            };
            writeln!(file, "From {sender} {stamp}")?;

            // ">From " escaping: a body line beginning "From " would otherwise
            // start a new message when the file is read back, silently splitting
            // one message into two.
            for line in raw.split(|b| *b == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.starts_with(b"From ") {
                    file.write_all(b">")?;
                }
                file.write_all(line)?;
                file.write_all(b"\n")?;
            }
            file.write_all(b"\n")?;
            written += 1;
        }
        file.flush()?;
        Ok((written, skipped))
    }

    /// Counts and bytes for the Storage pane.
    ///
    /// The index size comes from the FTS tables rather than being inferred from
    /// the file: the point of showing it separately is that it is the part you
    /// can rebuild, so a number that silently includes the mail is useless.
    pub fn storage_report(&self, db_path: &Path, blob_bytes: u64) -> Result<StorageReport> {
        let file = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let database_bytes = file(db_path.to_path_buf())
            + file(db_path.with_extension("db-wal"))
            + file(db_path.with_extension("db-shm"));

        // dbstat is a compile-time option; when it is missing the honest answer
        // is zero rather than a guess that would be wrong by an order of
        // magnitude either way.
        let index_bytes: u64 = self
            .conn
            .query_row(
                "SELECT coalesce(sum(pgsize), 0) FROM dbstat
                 WHERE name LIKE 'fts_%'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u64)
            .unwrap_or(0);

        Ok(StorageReport {
            messages: self.message_count()?,
            attachments: self
                .conn
                .query_row("SELECT count(*) FROM attachments", [], |r| r.get(0))?,
            database_bytes,
            blob_bytes,
            index_bytes,
        })
    }

    pub fn message_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?)
    }

    /// Conversations by most recent activity — the message list's real query.
    ///
    /// One row per thread, showing the newest message. `GROUP BY` after the
    /// join collapses ties where two messages share the newest timestamp,
    /// which would otherwise show a conversation twice.
    /// IMAP STORE semantics: add some flags, remove others, in one statement.
    /// Modelled on `+FLAGS`/`-FLAGS` rather than a whole-value setter because
    /// that is what the action queue has to replay against a server, and a
    /// read-modify-write here would lose a concurrent change from another client.
    pub fn set_flags(&self, message_id: i64, add: i64, remove: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET flags = (flags | ?2) & ~?3 WHERE id = ?1",
            params![message_id, add, remove],
        )?;
        Ok(())
    }

    pub fn set_has_attachments(&self, message_id: i64, yes: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET has_attachments = ?2 WHERE id = ?1",
            params![message_id, yes as i64],
        )?;
        Ok(())
    }

    /// Creates a tag if it is new, returning its id either way. Names are the
    /// provider's own (IMAP keyword, Gmail label), so they are matched exactly.
    pub fn ensure_tag(&self, account_id: i64, name: &str, colour: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO tags(account_id, name, colour) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, name) DO UPDATE SET colour = coalesce(excluded.colour, colour)",
            params![account_id, name, colour],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM tags WHERE account_id = ?1 AND name = ?2",
            params![account_id, name],
            |r| r.get(0),
        )?)
    }

    pub fn tag_message(&self, message_id: i64, tag_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO message_tags(message_id, tag_id) VALUES (?1, ?2)",
            params![message_id, tag_id],
        )?;
        Ok(())
    }

    pub fn untag_message(&self, message_id: i64, tag_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM message_tags WHERE message_id = ?1 AND tag_id = ?2",
            params![message_id, tag_id],
        )?;
        Ok(())
    }

    /// Tags for the rail, with how many conversations carry each.
    /// Renames a tag, keeping every message that carries it.
    ///
    /// The id is what a message is tagged with, so renaming is a change to one
    /// row and nothing has to be re-applied. Refused when the new name is
    /// already taken: two tags with one name are indistinguishable in the rail
    /// and in `tag:` searches, and merging them silently would be a decision
    /// the user did not ask for.
    pub fn rename_tag(&self, tag_id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Rejected("a tag needs a name".into()));
        }
        let clash: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM tags
                  WHERE account_id = (SELECT account_id FROM tags WHERE id = ?1)
                    AND lower(name) = lower(?2) AND id <> ?1",
                params![tag_id, name],
                |r| r.get(0),
            )
            .optional()?;
        if clash.is_some() {
            return Err(StoreError::Rejected(format!(
                "a tag called {name} already exists"
            )));
        }
        self.conn.execute(
            "UPDATE tags SET name = ?2 WHERE id = ?1",
            params![tag_id, name],
        )?;
        Ok(())
    }

    /// Sets a tag's colour, which is local to this machine by design — the
    /// providers have no field for it, so it is ours to keep and never syncs.
    pub fn set_tag_colour(&self, tag_id: i64, colour: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tags SET colour = ?2 WHERE id = ?1",
            params![tag_id, colour],
        )?;
        Ok(())
    }

    /// Removes a tag and takes it off every message carrying it.
    ///
    /// The rows in `message_tags` go with it rather than being left orphaned:
    /// a tag id pointing at nothing would show as a blank chip on the rows that
    /// still referenced it.
    pub fn delete_tag(&self, tag_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM message_tags WHERE tag_id = ?1", [tag_id])?;
        self.conn
            .execute("DELETE FROM tags WHERE id = ?1", [tag_id])?;
        Ok(())
    }

    pub fn tags_for_account(&self, account_id: i64) -> Result<Vec<TagSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.id, t.name, coalesce(t.colour,''),
                    count(DISTINCT coalesce(m.thread_id, -m.id))
             FROM tags t
             LEFT JOIN message_tags mt ON mt.tag_id = t.id
             LEFT JOIN messages m ON m.id = mt.message_id AND m.deleted_at_ms IS NULL
             WHERE t.account_id = ?1
             GROUP BY t.id ORDER BY t.name",
        )?;
        let rows = stmt.query_map(params![account_id], |row| {
            Ok(TagSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                colour: row.get(2)?,
                thread_count: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_threads(
        &self,
        view: &ListView,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ThreadListing>> {
        self.listing_rows(
            &view.predicate("messages"),
            &view.predicate("m"),
            limit,
            offset,
            view.bound().map(str::to_string),
        )
    }

    /// One conversation, found by its id rather than by looking for it in a
    /// view.
    ///
    /// A popped-out window knows which conversation it was opened for and
    /// nothing else about where it lives. Answering that by scanning a mailbox
    /// only works while the conversation happens to be in the mailbox guessed
    /// at — so a starred, archived, sent or merely old conversation opens into
    /// a window reporting it no longer exists.
    pub fn thread_by_id(&self, thread_id: i64) -> Result<Option<ThreadListing>> {
        // Bound as the same third parameter a view's folder role or tag name
        // would occupy, which is why this reads as a string.
        let rows = self.listing_rows(
            "coalesce(thread_id, -id) = cast(?3 AS INTEGER)",
            "coalesce(m.thread_id, -m.id) = cast(?3 AS INTEGER)",
            1,
            0,
            Some(thread_id.to_string()),
        )?;
        Ok(rows.into_iter().next())
    }

    /// The conversation-list query, shared by the views and by a lookup of one.
    fn listing_rows(
        &self,
        inner: &str,
        outer: &str,
        limit: u32,
        offset: u32,
        bound: Option<String>,
    ) -> Result<Vec<ThreadListing>> {
        let sql = format!(
            "SELECT coalesce(m.thread_id, -m.id), m.id, coalesce(m.from_display,''), coalesce(m.from_addr,''),
                    coalesce(m.subject,''), coalesce(m.snippet,''), m.date_ms, t.n,
                    coalesce(t.participants,''), t.unread, t.starred, t.attach,
                    (SELECT json_group_array(
                         json_object('id', tg.id, 'name', tg.name, 'colour', coalesce(tg.colour,'')))
                     FROM (SELECT DISTINCT mt.tag_id FROM message_tags mt
                           JOIN messages mm ON mm.id = mt.message_id
                           WHERE coalesce(mm.thread_id, -mm.id) = t.thread_id) d
                     JOIN tags tg ON tg.id = d.tag_id) AS tags_json,
                    (SELECT a.filename FROM attachments a
                     JOIN messages mm ON mm.id = a.message_id
                     WHERE coalesce(mm.thread_id, -mm.id) = t.thread_id
                       AND a.filename IS NOT NULL AND a.filename <> ''
                     ORDER BY a.id LIMIT 1) AS attach_name
             FROM messages m
             JOIN (
               SELECT coalesce(thread_id, -id) AS thread_id, max(date_ms) AS md, count(*) AS n,
                      group_concat(DISTINCT coalesce(nullif(from_display,''), from_addr))
                        AS participants,
                      max(CASE WHEN flags & 1 = 0 THEN 1 ELSE 0 END) AS unread,
                      max(CASE WHEN flags & 4 != 0 THEN 1 ELSE 0 END) AS starred,
                      max(has_attachments) AS attach
               FROM messages WHERE deleted_at_ms IS NULL AND {inner}
               GROUP BY coalesce(thread_id, -id)
             ) t ON coalesce(m.thread_id, -m.id) = t.thread_id AND m.date_ms = t.md
             WHERE m.deleted_at_ms IS NULL AND {outer}
             GROUP BY coalesce(m.thread_id, -m.id)
             ORDER BY m.date_ms DESC LIMIT ?1 OFFSET ?2",
            inner = inner,
            outer = outer,
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        // Two params, or three when the view binds a folder role or tag name.
        // rusqlite rejects a count that does not match what the SQL references,
        // so this cannot be a fixed tuple.
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(limit), Box::new(offset)];
        if let Some(b) = bound {
            args.push(Box::new(b));
        }
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |row| {
            Ok(ThreadListing {
                thread_id: row.get(0)?,
                id: row.get(1)?,
                from_display: row.get(2)?,
                from_addr: row.get(3)?,
                subject: row.get(4)?,
                snippet: row.get(5)?,
                date_ms: row.get(6)?,
                message_count: row.get(7)?,
                participants: row.get(8)?,
                unread: row.get::<_, i64>(9)? != 0,
                starred: row.get::<_, i64>(10)? != 0,
                has_attachments: row.get::<_, i64>(11)? != 0,
                tags: parse_row_tags(row.get::<_, Option<String>>(12)?),
                attachment_name: row.get::<_, Option<String>>(13)?,
                match_snippet: None,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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

    /// The numbers beside the rail's mailboxes.
    ///
    /// Counted per *conversation*, not per message: the list shows
    /// conversations, so a five-message thread holding one unread message is
    /// one unread row. A badge saying five would not match anything the user
    /// can point at.
    ///
    /// Not every mailbox counts the same thing, because "unread" does not mean
    /// the same thing everywhere. Mail you wrote is not unread in any useful
    /// sense — a Drafts badge stuck at zero while three drafts sit there would
    /// simply be wrong — so Drafts and the Outbox report how many are waiting.
    /// Sent reports nothing at all: there is no pending work in a sent message,
    /// and a number that never moves is furniture.
    pub fn view_counts(&self, mode: CountMode) -> Result<Vec<(String, i64)>> {
        // Mail you wrote yourself is never meaningfully unread, so these two
        // report how many are waiting even in unread mode — a Drafts badge
        // stuck at zero with three drafts sitting there would simply be wrong.
        const ALWAYS_TOTAL: [&str; 2] = ["drafts", "outbox"];
        const UNREAD: [&str; 6] = ["inbox", "starred", "snoozed", "archive", "spam", "trash"];

        let mut out = Vec::new();
        if mode == CountMode::Off {
            return Ok(out);
        }
        for key in UNREAD.iter().chain(ALWAYS_TOTAL.iter()).chain(
            // Sent only earns a number when the number is a total. There is no
            // pending work in a sent message, and a count that never moves is
            // furniture.
            ["sent"].iter().filter(|_| mode == CountMode::Total),
        ) {
            let view = ListView::parse(key);
            let total = mode == CountMode::Total || ALWAYS_TOTAL.contains(key);
            let n = self.count_view(&view, total)?;
            if n > 0 {
                out.push(((*key).to_string(), n));
            }
        }
        // Tags are left out: their rail rows already carry a count, and it is
        // a total rather than an unread one. "How much is tagged Urgent" is the
        // question a tag answers, and swapping it for an unread count would
        // lose that to make two different things look alike.
        Ok(out)
    }

    /// Conversations in a view: all of them, or only those holding something
    /// unread.
    fn count_view(&self, view: &ListView, total: bool) -> Result<i64> {
        let having = if total {
            String::new()
        } else {
            format!(
                "HAVING max(CASE WHEN flags & {seen} = 0 THEN 1 ELSE 0 END) = 1",
                seen = flags::SEEN
            )
        };
        let sql = format!(
            "SELECT count(*) FROM (
               SELECT coalesce(thread_id, -id) AS tid
               FROM messages
               WHERE deleted_at_ms IS NULL AND {pred}
               GROUP BY coalesce(thread_id, -id)
               {having}
             )",
            pred = view.predicate("messages"),
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        // The predicate binds its folder role or tag name as ?3, so the two
        // lower slots have to exist even though nothing reads them.
        Ok(match view.bound() {
            Some(b) => stmt.query_row(
                rusqlite::params![rusqlite::types::Null, rusqlite::types::Null, b],
                |r| r.get(0),
            )?,
            None => stmt.query_row([], |r| r.get(0))?,
        })
    }

    /// Whether this message's remote content may load.
    ///
    /// Blocked unless one of two things is true:
    ///
    /// * the sender has been trusted explicitly, or
    /// * the user has written to them.
    ///
    /// The second is the one that makes blocking liveable. A tracking pixel
    /// buys the sender three facts — the address is real, when it was read, and
    /// roughly from where. Someone the user has already emailed has the first
    /// two by other means and is not a stranger, so the trade is no longer
    /// worth breaking their mail over. It is deliberately a question about sent
    /// mail rather than a stored flag, so it answers correctly for a
    /// correspondent from before this feature existed, and stops being true if
    /// that mail is ever removed.
    pub fn remote_content_allowed(&self, message_id: i64) -> Result<bool> {
        let row: Option<(i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT account_id, from_addr FROM messages WHERE id = ?1",
                params![message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        // No sender to judge means no reason to trust it.
        let Some((account_id, Some(from))) = row else {
            return Ok(false);
        };
        let from = from.trim().to_lowercase();
        if from.is_empty() {
            return Ok(false);
        }
        Ok(self.sender_trusted(account_id, &from)? || self.has_written_to(account_id, &from)?)
    }

    /// Addresses to offer while a recipient is being typed, best first.
    ///
    /// Harvested from mail already synced — there is no lookup anywhere else,
    /// which is the point: a composer that phones a server to ask who you might
    /// be writing to has told it who you are writing to.
    ///
    /// Ranked by frecency rather than either half alone. Frequency by itself
    /// buries the person you emailed twice this morning under a mailing list
    /// from three years ago; recency by itself puts whoever mailed you last
    /// above your closest colleague. The weights below are deliberately blunt:
    ///
    /// * **Written to, hugely.** Somebody you have sent mail to is a different
    ///   kind of thing from an address that has appeared in your inbox, and no
    ///   amount of newsletter volume should outrank one.
    /// * **Seen often**, linearly, capped so a mailing list cannot dominate.
    /// * **Seen lately**, in two coarse steps rather than a curve — the
    ///   difference between last week and last month matters, the difference
    ///   between fourteen and fifteen months does not.
    ///
    /// Spam is excluded, and so is the user's own address: a completion list
    /// that offers to send mail to yourself is answering a question nobody
    /// asked.
    pub fn complete_addresses(
        &self,
        account_id: i64,
        prefix: &str,
        now_ms: i64,
        limit: u32,
    ) -> Result<Vec<Correspondent>> {
        let needle = prefix.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        // Matched at the start of the address, of its domain, or of any word in
        // the display name — the three places people begin typing. Not a bare
        // substring: "an" should not offer everyone with "an" mid-surname.
        let starts = format!("{needle}%");
        let word = format!("% {needle}%");
        let at = format!("%@{needle}%");
        let month = now_ms - 30 * crate::retention::MS_PER_DAY;
        let quarter = now_ms - 90 * crate::retention::MS_PER_DAY;

        let mut stmt = self.conn.prepare_cached(
            "SELECT ma.addr_norm,
                    coalesce(max(nullif(ma.display, '')), '') AS display,
                    max(CASE WHEN EXISTS (
                          SELECT 1 FROM placements p
                          JOIN folders f ON f.id = p.folder_id
                          WHERE p.message_id = m.id AND f.role = 'sent')
                        AND ma.role IN ('to', 'cc')
                        THEN 1 ELSE 0 END) AS written,
                    count(*) AS seen,
                    max(m.date_ms) AS last_ms
             FROM message_addresses ma
             JOIN messages m ON m.id = ma.message_id
             WHERE m.account_id = ?1
               AND m.deleted_at_ms IS NULL
               AND ma.addr_norm <> ''
               AND NOT EXISTS (SELECT 1 FROM placements p
                               JOIN folders f ON f.id = p.folder_id
                               WHERE p.message_id = m.id AND f.role = 'spam')
               AND ma.addr_norm <> coalesce(
                     (SELECT lower(email) FROM accounts WHERE id = ?1), '')
               AND (ma.addr_norm LIKE ?2
                    OR lower(ma.display) LIKE ?2
                    OR lower(ma.display) LIKE ?3
                    OR ma.addr_norm LIKE ?4)
             GROUP BY ma.addr_norm
             ORDER BY written DESC,
                      min(seen, 20)
                        + CASE WHEN last_ms > ?5 THEN 25
                               WHEN last_ms > ?6 THEN 10
                               ELSE 0 END DESC,
                      last_ms DESC
             LIMIT ?7",
        )?;
        let rows = stmt.query_map(
            params![account_id, starts, word, at, month, quarter, limit],
            |r| {
                Ok(Correspondent {
                    addr: r.get(0)?,
                    display: r.get(1)?,
                    written_to: r.get::<_, i64>(2)? != 0,
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Who sent a message, normalised the way the trust list stores addresses.
    pub fn message_sender(&self, message_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT from_addr FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|a| a.trim().to_lowercase())
            .filter(|a| !a.is_empty()))
    }

    /// Whether this address was trusted by hand.
    pub fn sender_trusted(&self, account_id: i64, addr: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM remote_senders
                            WHERE account_id = ?1 AND addr_norm = ?2)",
            params![account_id, addr.trim().to_lowercase()],
            |r| r.get::<_, i64>(0),
        )? != 0)
    }

    /// Whether the user has ever sent mail to this address.
    ///
    /// Sent mail only — being written *to* by someone says nothing about
    /// whether they know you, which is the entire question.
    pub fn has_written_to(&self, account_id: i64, addr: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM message_addresses ma
               JOIN messages m ON m.id = ma.message_id
               JOIN placements p ON p.message_id = m.id
               JOIN folders f ON f.id = p.folder_id
               WHERE m.account_id = ?1
                 AND m.deleted_at_ms IS NULL
                 AND f.role = 'sent'
                 AND ma.role IN ('to', 'cc')
                 AND ma.addr_norm = ?2)",
            params![account_id, addr.trim().to_lowercase()],
            |r| r.get::<_, i64>(0),
        )? != 0)
    }

    /// Trusts a sender's remote content from now on.
    pub fn trust_sender(&self, account_id: i64, addr: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_senders(account_id, addr_norm, added_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, addr_norm) DO UPDATE SET added_ms = excluded.added_ms",
            params![account_id, addr.trim().to_lowercase(), now_ms],
        )?;
        Ok(())
    }

    /// Takes that trust back.
    pub fn untrust_sender(&self, account_id: i64, addr: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM remote_senders WHERE account_id = ?1 AND addr_norm = ?2",
            params![account_id, addr.trim().to_lowercase()],
        )?;
        Ok(())
    }

    /// The trusted senders, most recently trusted first — what the privacy
    /// pane lists so a decision made once in a banner can be found and undone.
    pub fn trusted_senders(&self, account_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT addr_norm FROM remote_senders WHERE account_id = ?1
             ORDER BY added_ms DESC, addr_norm",
        )?;
        let rows = stmt.query_map(params![account_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Files messages where Gmail says they are.
    ///
    /// Over plain IMAP a message is only ever "in" the mailbox it was fetched
    /// from, so archived — which on Gmail means *not carrying the Inbox label*
    /// — is not something the protocol can express, and Petrel could only infer
    /// it. These are Gmail's own labels, so the inference goes away.
    ///
    /// Local work outranks the server. A message with a queued action has been
    /// moved by the user and not yet delivered; taking the server's older
    /// opinion would undo it on screen and then send the undo.
    ///
    /// Returns how many were refiled.
    pub fn apply_gmail_labels(
        &self,
        account_id: i64,
        labelled: &[(String, Vec<String>)],
    ) -> Result<usize> {
        // The label arrives quoted and how many backslashes survive is a detail
        // of the parser, so match on the name rather than the escaping.
        let has = |ls: &[String], name: &str| ls.iter().any(|l| l.ends_with(name));

        let inbox = self.ensure_folder(account_id, "inbox", "INBOX")?;
        let archive = self.ensure_folder(account_id, "archive", "archive")?;
        let mut changed = 0usize;

        for (msg_id, labels) in labelled {
            let existing: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM messages
                     WHERE account_id = ?1 AND message_id_hdr = ?2 AND deleted_at_ms IS NULL",
                    params![account_id, msg_id],
                    |r| r.get(0),
                )
                .optional()?;
            // Not held. Knowing where a message we do not have lives is not
            // worth a row we could not open.
            let Some(id) = existing else { continue };
            if self.message_has_pending(id)? {
                continue;
            }

            let in_inbox = has(labels, "Inbox");
            let (add, drop) = if in_inbox {
                (inbox, archive)
            } else {
                (archive, inbox)
            };
            self.conn.execute(
                "DELETE FROM placements WHERE message_id = ?1 AND folder_id = ?2",
                params![id, drop],
            )?;
            self.place_message(id, add)?;

            // Starred is a flag rather than a place, and the same sweep carries
            // it — which is the whole reason a star on old mail was invisible.
            if has(labels, "Starred") {
                self.set_flags(id, flags::FLAGGED, 0)?;
            } else {
                self.set_flags(id, 0, flags::FLAGGED)?;
            }
            changed += 1;
        }
        Ok(changed)
    }

    /// Every live message in one conversation, oldest first — the reading pane
    /// renders these in order with earlier ones collapsed.
    /// Search, rolled up to conversations. A query matches a *message*, but the
    /// list shows conversations, so hits are resolved to their threads with
    /// duplicates collapsed — otherwise a five-message thread where four match
    /// would fill the results with itself. Rank order is preserved: the thread
    /// takes the position of its best-matching message.
    /// How results are ordered.
    ///
    /// Best match by default. Ranking is local BM25 over the extracted text, so
    /// the thing you are looking for is usually first — sorting by date is for
    /// retracing a timeline, which is a different question and one click away.
    pub fn search_threads_sorted(
        &self,
        query: &str,
        limit: u32,
        newest_first: bool,
    ) -> Result<Vec<ThreadListing>> {
        let mut rows = self.search_threads(query, limit)?;
        if newest_first {
            rows.sort_by(|a, b| b.date_ms.cmp(&a.date_ms));
        }
        Ok(rows)
    }

    pub fn search_threads(&self, query: &str, limit: u32) -> Result<Vec<ThreadListing>> {
        let q = crate::search_query::parse(query);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let wide = limit.saturating_mul(3).min(600);

        // Words rank; conditions filter. With no words there is nothing for
        // BM25 to score, so `has:attachment` on its own is a listing in date
        // order — which is the right answer to a question that named no terms.
        let hits: Vec<Listing> = if q.text.trim().is_empty() {
            self.messages_meeting(&q, wide)?
        } else {
            let found = self.search_listing(&q.text, wide)?;
            let keep = self.ids_meeting(&found.iter().map(|h| h.id).collect::<Vec<_>>(), &q)?;
            found.into_iter().filter(|h| keep.contains(&h.id)).collect()
        };
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let mut order: Vec<i64> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // The first hit for a conversation is its best one — the list arrives
        // ranked — so that is the snippet the row shows.
        let mut why: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
        for h in &hits {
            let tid = self.thread_of(h.id)?.unwrap_or(-h.id);
            if seen.insert(tid) {
                order.push(tid);
                if !h.snippet.is_empty() {
                    why.insert(tid, h.snippet.clone());
                }
            }
        }
        order.truncate(limit as usize);
        if order.is_empty() {
            return Ok(Vec::new());
        }

        let mut rows = self.threads_by_id(&order)?;
        // Restore rank order — SQL gave us the rows, not the ranking.
        let rank: std::collections::HashMap<i64, usize> =
            order.iter().enumerate().map(|(i, t)| (*t, i)).collect();
        rows.sort_by_key(|r| rank.get(&r.thread_id).copied().unwrap_or(usize::MAX));
        for row in &mut rows {
            row.match_snippet = why.remove(&row.thread_id);
        }
        Ok(rows)
    }

    /// The SQL for a query's conditions, and the values they bind.
    ///
    /// Built rather than interpolated: `from:` and `in:` carry whatever was
    /// typed, and a search box that reaches SQL is the oldest mistake there is.
    fn conditions(q: &crate::search_query::SearchQuery) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        let mut sql = String::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        // Junk and deleted mail stay out unless they are what was asked for.
        //
        // Searching a mailbox is not an invitation to reopen what was already
        // judged and discarded, and a result that quietly comes from Spam is
        // worse than no result at all: it puts a message the filter rejected
        // back in front of the reader looking exactly like ordinary mail. The
        // grammar is the way in — `in:spam` and `in:trash` search them, and
        // nothing else does.
        if !matches!(q.in_role.as_deref(), Some("spam") | Some("trash")) {
            sql.push_str(&format!(" AND {}", not_binned("m")));
        }
        if let Some(from) = &q.from {
            sql.push_str(
                " AND (lower(coalesce(m.from_addr,'')) LIKE ?
                       OR lower(coalesce(m.from_display,'')) LIKE ?)",
            );
            let like = format!("%{}%", from.to_lowercase());
            args.push(Box::new(like.clone()));
            args.push(Box::new(like));
        }
        if q.has_attachment {
            sql.push_str(" AND m.has_attachments = 1");
        }
        if q.unread {
            sql.push_str(&format!(" AND m.flags & {} = 0", flags::SEEN));
        }
        if q.starred {
            sql.push_str(&format!(" AND m.flags & {} != 0", flags::FLAGGED));
        }
        if let Some(role) = &q.in_role {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM placements p JOIN folders f ON f.id = p.folder_id
                              WHERE p.message_id = m.id AND f.role = ?)",
            );
            args.push(Box::new(role.clone()));
        }
        if let Some(after) = q.after_ms {
            sql.push_str(" AND m.date_ms >= ?");
            args.push(Box::new(after));
        }
        (sql, args)
    }

    /// Which of these messages meet the query's conditions.
    fn ids_meeting(
        &self,
        ids: &[i64],
        q: &crate::search_query::SearchQuery,
    ) -> Result<std::collections::HashSet<i64>> {
        if ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let (conds, mut args) = Self::conditions(q);
        if conds.is_empty() {
            return Ok(ids.iter().copied().collect());
        }
        let holes = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT m.id FROM messages m
             WHERE m.deleted_at_ms IS NULL AND m.id IN ({holes}){conds}"
        );
        let mut all: Vec<Box<dyn rusqlite::ToSql>> = ids
            .iter()
            .map(|i| Box::new(*i) as Box<dyn rusqlite::ToSql>)
            .collect();
        all.append(&mut args);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(all), |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<std::result::Result<std::collections::HashSet<_>, _>>()?)
    }

    /// Every message meeting the conditions, newest first — the answer when a
    /// query named conditions but no words.
    fn messages_meeting(
        &self,
        q: &crate::search_query::SearchQuery,
        limit: u32,
    ) -> Result<Vec<Listing>> {
        let (conds, mut args) = Self::conditions(q);
        let sql = format!(
            "SELECT m.id, coalesce(m.from_display,''), coalesce(m.from_addr,''),
                    coalesce(m.subject,''), m.date_ms
             FROM messages m
             WHERE m.deleted_at_ms IS NULL{conds}
             ORDER BY m.date_ms DESC LIMIT ?"
        );
        args.push(Box::new(limit));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok(Listing {
                id: r.get(0)?,
                from_display: r.get(1)?,
                from_addr: r.get(2)?,
                subject: r.get(3)?,
                date_ms: r.get(4)?,
                // Nothing was searched for, so there is nothing to mark.
                snippet: String::new(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The thread-row aggregate, restricted to a set of conversations.
    fn threads_by_id(&self, thread_ids: &[i64]) -> Result<Vec<ThreadListing>> {
        let holes = std::iter::repeat_n("?", thread_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT coalesce(m.thread_id, -m.id), m.id, coalesce(m.from_display,''), coalesce(m.from_addr,''),
                    coalesce(m.subject,''), coalesce(m.snippet,''), m.date_ms, t.n,
                    coalesce(t.participants,''), t.unread, t.starred, t.attach,
                    (SELECT json_group_array(json_object('name', tg.name, 'colour', coalesce(tg.colour,'')))
                     FROM (SELECT DISTINCT mt.tag_id FROM message_tags mt
                           JOIN messages mm ON mm.id = mt.message_id
                           WHERE coalesce(mm.thread_id, -mm.id) = t.thread_id) d
                     JOIN tags tg ON tg.id = d.tag_id) AS tags_json,
                    (SELECT a.filename FROM attachments a
                     JOIN messages mm ON mm.id = a.message_id
                     WHERE coalesce(mm.thread_id, -mm.id) = t.thread_id
                       AND a.filename IS NOT NULL AND a.filename <> ''
                     ORDER BY a.id LIMIT 1) AS attach_name
             FROM messages m
             JOIN (
               SELECT coalesce(thread_id, -id) AS thread_id, max(date_ms) AS md, count(*) AS n,
                      group_concat(DISTINCT coalesce(nullif(from_display,''), from_addr))
                        AS participants,
                      max(CASE WHEN flags & 1 = 0 THEN 1 ELSE 0 END) AS unread,
                      max(CASE WHEN flags & 4 != 0 THEN 1 ELSE 0 END) AS starred,
                      max(has_attachments) AS attach
               FROM messages WHERE deleted_at_ms IS NULL
               GROUP BY coalesce(thread_id, -id)
             ) t ON coalesce(m.thread_id, -m.id) = t.thread_id AND m.date_ms = t.md
             WHERE m.deleted_at_ms IS NULL AND coalesce(m.thread_id, -m.id) IN ({holes})
             GROUP BY coalesce(m.thread_id, -m.id)"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(thread_ids), |row| {
            Ok(ThreadListing {
                thread_id: row.get(0)?,
                id: row.get(1)?,
                from_display: row.get(2)?,
                from_addr: row.get(3)?,
                subject: row.get(4)?,
                snippet: row.get(5)?,
                date_ms: row.get(6)?,
                message_count: row.get(7)?,
                participants: row.get(8)?,
                unread: row.get::<_, i64>(9)? != 0,
                starred: row.get::<_, i64>(10)? != 0,
                has_attachments: row.get::<_, i64>(11)? != 0,
                tags: parse_row_tags(row.get::<_, Option<String>>(12)?),
                attachment_name: row.get::<_, Option<String>>(13)?,
                match_snippet: None,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// One conversation, message by message, with what the reading pane needs to
    /// draw a card per message: who it came from, who it went to, and its files.
    pub fn thread_detail(&self, thread_id: i64) -> Result<Vec<ThreadMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), coalesce(snippet,''), date_ms, flags
             FROM messages
             WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL
             ORDER BY date_ms ASC",
        )?;
        let rows: Vec<(i64, String, String, String, String, i64, i64)> = stmt
            .query_map(params![thread_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (id, from_display, from_addr, subject, snippet, date_ms, flags) in rows {
            let mut to = self.conn.prepare_cached(
                // message_addresses has no surrogate key; rowid preserves the
                // order the parser inserted them, which is the header's order.
                "SELECT coalesce(nullif(display,''), addr_norm), addr_norm
                 FROM message_addresses
                 WHERE message_id = ?1 AND role IN ('to','cc') ORDER BY rowid",
            )?;
            let pairs: Vec<(String, String)> = to
                .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let recipients: Vec<String> = pairs.iter().map(|(d, _)| d.clone()).collect();
            let recipient_addrs: Vec<String> = pairs.into_iter().map(|(_, a)| a).collect();

            let mut att = self.conn.prepare_cached(
                "SELECT coalesce(filename,''), coalesce(size, 0) FROM attachments
                 WHERE message_id = ?1 AND filename IS NOT NULL AND filename <> ''
                 ORDER BY id",
            )?;
            let attachments: Vec<Attachment> = att
                .query_map(params![id], |r| {
                    Ok(Attachment {
                        filename: r.get(0)?,
                        size: r.get(1)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            out.push(ThreadMessage {
                id,
                from_display,
                from_addr,
                subject,
                snippet,
                date_ms,
                unread: flags & flags::SEEN == 0,
                recipients,
                recipient_addrs,
                attachments,
            });
        }
        Ok(out)
    }

    pub fn messages_in_thread(&self, thread_id: i64) -> Result<Vec<Listing>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), coalesce(snippet,''), date_ms
             FROM messages WHERE thread_id = ?1 AND deleted_at_ms IS NULL
             ORDER BY date_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![thread_id], |row| {
            Ok(Listing {
                id: row.get(0)?,
                from_display: row.get(1)?,
                from_addr: row.get(2)?,
                subject: row.get(3)?,
                snippet: row.get(4)?,
                date_ms: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The conversation a message belongs to.
    pub fn thread_of(&self, message_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT thread_id FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten())
    }

    /// Most-recent-activity page for list surfaces.
    pub fn list_recent(&self, offset: u32, limit: u32) -> Result<Vec<Listing>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), coalesce(snippet,''), date_ms
             FROM messages WHERE deleted_at_ms IS NULL
             ORDER BY date_ms DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |row| {
            Ok(Listing {
                id: row.get(0)?,
                from_display: row.get(1)?,
                from_addr: row.get(2)?,
                subject: row.get(3)?,
                snippet: row.get(4)?,
                date_ms: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Search results joined with display metadata; the snippet carries
    /// `[`…`]` highlight markers from FTS5.
    pub fn search_listing(&self, query: &str, limit: u32) -> Result<Vec<Listing>> {
        let hits = self.search(query, limit)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), date_ms
             FROM messages WHERE id = ?1",
        )?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let row = stmt.query_row(params![h.message_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            out.push(Listing {
                id: h.message_id,
                from_display: row.0,
                from_addr: row.1,
                subject: row.2,
                snippet: h.snippet,
                date_ms: row.3,
            });
        }
        Ok(out)
    }

    pub fn db_size_bytes(&self) -> Result<i64> {
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(pages * page_size)
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
