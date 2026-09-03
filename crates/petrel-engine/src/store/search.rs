//! Search: the grammar's conditions bound into SQL, FTS hits resolved to
//! conversations, and the account wall on all of it.
//!
//! Moved verbatim from mod.rs (Phase 1.5); the free helpers it leans on —
//! match_expr, the CJK machinery, in_inbox — remain in mod.rs and arrive
//! through `use super::*`.
use super::*;

impl Store {
    /// Routed search: CJK queries use the per-character index, everything else
    /// the unicode61 index with as-you-type prefix on the final token.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
        self.search_page(query, limit, 0)
    }

    /// One page of the ranking, skipping the `offset` best matches.
    ///
    /// A search is scoped to the account on screen, and the account is not
    /// something FTS5 knows: the filter is applied to the hits afterwards.
    /// One page of hits is therefore not one page of results, and taking a
    /// single fixed slice of the ranking meant a word common in the other
    /// account could fill it entirely — six hundred short matches over
    /// there hid the one match here, and the search box said there was
    /// nothing. Paging is what lets the caller keep asking until it has
    /// enough of its own.
    pub fn search_page(&self, query: &str, limit: u32, offset: u32) -> Result<Vec<SearchHit>> {
        if query.chars().any(is_cjk) {
            self.search_cjk_page(query, limit, offset)
        } else {
            self.search_unicode_page(query, limit, offset)
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
        self.search_unicode_page(query, limit, 0)
    }

    fn search_unicode_page(&self, query: &str, limit: u32, offset: u32) -> Result<Vec<SearchHit>> {
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
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![expr, limit, offset], |row| {
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
        self.search_cjk_page(query, limit, 0)
    }

    fn search_cjk_page(&self, query: &str, limit: u32, offset: u32) -> Result<Vec<SearchHit>> {
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
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![expr, limit, offset], |row| {
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
    /// Search results in a chosen order, or in the order the ranking put them.
    ///
    /// `None` is best match — the one order only a search can offer, because
    /// only a search has a query to be relevant to. Everything else is the
    /// same three keys a list offers, applied to the rows the search already
    /// found rather than to the mailbox, so the cost is the result set and not
    /// the account.
    pub fn search_threads_sorted(
        &self,
        query: &str,
        limit: u32,
        sort: Option<Sort>,
    ) -> Result<Vec<ThreadListing>> {
        let mut rows = self.search_threads(query, limit)?;
        let Some(sort) = sort else { return Ok(rows) };
        match sort.key {
            SortKey::Date => rows.sort_by_key(|r| r.date_ms),
            SortKey::Sender => rows.sort_by_key(|r| {
                let who = if r.from_display.is_empty() {
                    &r.from_addr
                } else {
                    &r.from_display
                };
                who.to_lowercase()
            }),
            SortKey::Subject => rows.sort_by_key(|r| r.subject.to_lowercase()),
        }
        if !sort.ascending {
            rows.reverse();
        }
        Ok(rows)
    }

    pub fn search_threads(&self, query: &str, limit: u32) -> Result<Vec<ThreadListing>> {
        let q = crate::search_query::parse(query);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        // No account, no results — never "everyone's results".
        let Some(account) = self.active_account()? else {
            return Ok(Vec::new());
        };
        // A page of the ranking: wide enough that one round usually
        // answers, small enough that a query matching most of a mailbox
        // does not read it all before filtering.
        let wide = limit.saturating_mul(3).clamp(50, 600);

        let mut order: Vec<i64> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // The first hit for a conversation is its best one — the list arrives
        // ranked — so that is the snippet the row shows.
        let mut why: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
        // Words rank; conditions filter. With no words there is nothing for
        // BM25 to score, so `has:attachment` on its own is a listing in date
        // order — which is the right answer to a question that named no terms.
        if q.text.trim().is_empty() {
            let hits = self.messages_meeting(&q, wide, account)?;
            self.gather_threads(&hits, &mut seen, &mut order, &mut why)?;
        } else {
            // The account filter is applied after the ranking, so a page of
            // hits is not a page of results: the ranking is walked a page at
            // a time until this account has enough conversations of its own.
            // The cap is what keeps a word that matches most of a large
            // mailbox from walking all of it — the answer is then the best
            // matches rather than every match, which is what a search is.
            const SCAN_CAP: u32 = 12_000;
            let mut offset = 0u32;
            loop {
                let found = self.search_listing_page(&q.text, wide, offset)?;
                let exhausted = (found.len() as u32) < wide;
                let ids: Vec<i64> = found.iter().map(|h| h.id).collect();
                let keep = self.ids_meeting(&ids, &q, account)?;
                let kept: Vec<Listing> =
                    found.into_iter().filter(|h| keep.contains(&h.id)).collect();
                self.gather_threads(&kept, &mut seen, &mut order, &mut why)?;
                offset = offset.saturating_add(wide);
                if exhausted || order.len() >= limit as usize || offset >= SCAN_CAP {
                    break;
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

    /// Folds a page of ranked hits into the conversations they belong to,
    /// keeping the order the ranking put them in and the snippet of each
    /// conversation's best-matching message.
    fn gather_threads(
        &self,
        hits: &[Listing],
        seen: &mut std::collections::HashSet<i64>,
        order: &mut Vec<i64>,
        why: &mut std::collections::HashMap<i64, String>,
    ) -> Result<()> {
        for h in hits {
            let tid = self.thread_of(h.id)?.unwrap_or(-h.id);
            if seen.insert(tid) {
                order.push(tid);
                if !h.snippet.is_empty() {
                    why.insert(tid, h.snippet.clone());
                }
            }
        }
        Ok(())
    }

    /// The SQL for a query's conditions, and the values they bind.
    ///
    /// Built rather than interpolated: `from:` and `in:` carry whatever was
    /// typed, and a search box that reaches SQL is the oldest mistake there is.
    fn conditions(
        q: &crate::search_query::SearchQuery,
        account: i64,
    ) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        let mut sql = String::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        // The account on screen, always. Search used to run over the whole
        // store, so standing in one account quietly answered with the other
        // account's mail — the one wall the multi-account design promises
        // never leaks (03 §4.1), broken precisely where it is least visible.
        sql.push_str(" AND m.account_id = ?");
        args.push(Box::new(account));

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
        if q.snoozed {
            sql.push_str(" AND coalesce(m.snoozed_until_ms, 0) > (strftime('%s','now') * 1000)");
        }
        if let Some(name) = &q.in_role {
            // A role, or a folder the user made — by full path or by leaf, so
            // `in:receipts` and `in:projects/petrel` both say what they mean.
            // The parser lowercased the value; the comparisons follow suit.
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM placements p JOIN folders f ON f.id = p.folder_id
                              WHERE p.message_id = m.id
                                AND (f.role = ?
                                     OR lower(f.path) = ?
                                     OR lower(f.path) LIKE '%/' || ?
                                     OR lower(f.path) LIKE '%.' || ?))",
            );
            for _ in 0..4 {
                args.push(Box::new(name.clone()));
            }
            // Snoozing takes a message out of the inbox until it comes back.
            // That is what the Inbox view's predicate says and what its unread
            // badge counts, and search used to disagree: `in:inbox is:unread`
            // returned the snoozed ones too, so the list and the number beside
            // the mailbox differed by exactly the mail somebody had put off.
            //
            // Only the inbox, because that is the only view snoozing hides
            // from — and not when `is:snoozed` asked for them by name, since
            // snoozing hides mail rather than burying it.
            if name == "inbox" && !q.snoozed {
                sql.push_str(
                    " AND coalesce(m.snoozed_until_ms, 0) <= (strftime('%s','now') * 1000)",
                );
            }
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
        account: i64,
    ) -> Result<std::collections::HashSet<i64>> {
        if ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let (conds, mut args) = Self::conditions(q, account);
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
        account: i64,
    ) -> Result<Vec<Listing>> {
        let (conds, mut args) = Self::conditions(q, account);
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

    /// Search results joined with display metadata; the snippet carries
    /// `[`…`]` highlight markers from FTS5.
    pub fn search_listing(&self, query: &str, limit: u32) -> Result<Vec<Listing>> {
        self.search_listing_page(query, limit, 0)
    }

    /// `search_listing`, skipping the `offset` best matches — one page of the
    /// ranking for a caller that filters the hits itself.
    fn search_listing_page(&self, query: &str, limit: u32, offset: u32) -> Result<Vec<Listing>> {
        let hits = self.search_page(query, limit, offset)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), date_ms
             FROM messages WHERE id = ?1",
        )?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            // A hit whose message row is gone is an index row nothing owns.
            // Skipped, not fatal: one stray row used to fail every search
            // that matched it, for good.
            let Some(row) = stmt
                .query_row(params![h.message_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .optional()?
            else {
                continue;
            };
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
}
