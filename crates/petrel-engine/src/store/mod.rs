//! The metadata + search store: one SQLite database (WAL), fat rows, and FTS5
//! indexes over a dedicated extracted-text table (`fts_content`).
//!
//! Invariant: `fts_content` is written in the same transaction as its message
//! row, and only through this API. Index consistency is verifiable at any time
//! via [`Store::fts_integrity_check`] and repairable via [`Store::rebuild_fts`].

use crate::retention::RetentionMode;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, params};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 2;
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
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub colour: String,
    pub thread_count: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThreadRowTag {
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
    out.push('[');
    out.extend(&chars[at..at + n]);
    out.push(']');
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
        let snippet: String = index_text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
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
                tx.execute(
                    "INSERT OR REPLACE INTO placements(message_id, folder_id, uid) VALUES (?1, ?2, ?3)",
                    params![id, fid, uid],
                )?;
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

    pub fn search_unicode(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        let Some(expr) = match_expr(query, true) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT rowid,
                    bm25(fts_messages, 4.0, 1.0, 2.0, 2.0) AS r,
                    snippet(fts_messages, 1, '[', ']', '…', 12)
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

    pub fn list_threads(&self, offset: u32, limit: u32) -> Result<Vec<ThreadListing>> {
        let mut stmt = self.conn.prepare_cached(
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
             WHERE m.deleted_at_ms IS NULL
             GROUP BY coalesce(m.thread_id, -m.id)
             ORDER BY m.date_ms DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |row| {
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
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Every live message in one conversation, oldest first — the reading pane
    /// renders these in order with earlier ones collapsed.
    /// Search, rolled up to conversations. A query matches a *message*, but the
    /// list shows conversations, so hits are resolved to their threads with
    /// duplicates collapsed — otherwise a five-message thread where four match
    /// would fill the results with itself. Rank order is preserved: the thread
    /// takes the position of its best-matching message.
    pub fn search_threads(&self, query: &str, limit: u32) -> Result<Vec<ThreadListing>> {
        let hits = self.search_listing(query, limit.saturating_mul(3).min(600))?;
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let mut order: Vec<i64> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for h in &hits {
            let tid = self.thread_of(h.id)?.unwrap_or(-h.id);
            if seen.insert(tid) {
                order.push(tid);
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
        Ok(rows)
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
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
