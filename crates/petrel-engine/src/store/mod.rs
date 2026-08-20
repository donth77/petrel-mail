//! The metadata + search store: one SQLite database (WAL), fat rows, and FTS5
//! indexes over a dedicated extracted-text table (`fts_content`).
//!
//! Invariant: `fts_content` is written in the same transaction as its message
//! row, and only through this API. Index consistency is verifiable at any time
//! via [`Store::fts_integrity_check`] and repairable via [`Store::rebuild_fts`].

use rusqlite::{Connection, params};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;
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

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver < SCHEMA_VERSION {
            conn.execute_batch(include_str!("schema.sql"))?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('extractor_version', ?1)",
                params![EXTRACTOR_VERSION.to_string()],
            )?;
        }
        Ok(Store { conn })
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

    /// Routed search: CJK queries use the trigram index, everything else the
    /// unicode61 index with as-you-type prefix on the final token.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        if query.chars().any(is_cjk) {
            self.search_trigram(query, limit)
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

    pub fn search_trigram(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        let Some(expr) = match_expr(query, false) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT rowid,
                    bm25(fts_trigram, 4.0, 1.0) AS r,
                    snippet(fts_trigram, 1, '[', ']', '…', 12)
             FROM fts_trigram
             WHERE fts_trigram MATCH ?1
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

    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('rebuild')",
            [],
        )?;
        self.conn
            .execute("INSERT INTO fts_trigram(fts_trigram) VALUES('rebuild')", [])?;
        Ok(())
    }

    pub fn optimize_fts(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('optimize')",
            [],
        )?;
        self.conn.execute(
            "INSERT INTO fts_trigram(fts_trigram) VALUES('optimize')",
            [],
        )?;
        Ok(())
    }

    /// Verifies each FTS index against `fts_content`; errors on divergence.
    pub fn fts_integrity_check(&self) -> Result<()> {
        for t in ["fts_messages", "fts_trigram"] {
            let sql = format!("INSERT INTO {t}({t}) VALUES('integrity-check')");
            if let Err(e) = self.conn.execute(&sql, []) {
                return Err(StoreError::Integrity(format!("{t}: {e}")));
            }
        }
        Ok(())
    }

    pub fn message_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?)
    }

    /// Most-recent-activity page for list surfaces.
    pub fn list_recent(&self, offset: u32, limit: u32) -> Result<Vec<Listing>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), coalesce(snippet,''), date_ms
             FROM messages ORDER BY date_ms DESC LIMIT ?1 OFFSET ?2",
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
