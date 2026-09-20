//! Search: hits resolved to conversations, with the account wall on all of
//! it.
//!
//! Three steps, one to a file, because the first two can be read and tested
//! without a mailbox and this one cannot:
//!
//! - `plan` works out what the query asks for: how many statements it
//!   becomes, which index each is asked of, which word is still being typed.
//!   Pure, and unit-tested in place.
//! - `sql` writes a statement out, with every value bound.
//! - here: the statements are run, their hits rolled up into conversations,
//!   and the rows fetched with the snippet that says why each matched.
//!
//! The free helpers this leans on — match_expr, the CJK machinery, in_inbox —
//! live in mod.rs and arrive through `use super::*`, which is also how they
//! reach the two files beside this one.
use super::*;
use crate::search_query::{Expr, MAX_ALTERNATIVES, State, Term, Text};

mod plan;
mod sql;

use plan::*;
use sql::*;

/// A message a search found, before anyone has asked why.
struct Hit {
    id: i64,
    /// Only a listing is ordered by date; a ranked hit leaves this at zero.
    date_ms: i64,
    /// Which alternative's words found it — what its snippet is made with.
    /// `None` when it met conditions and no words were asked.
    from: Option<usize>,
}

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
        let (hits, asked) = self.hits_meeting(&q, wide, account)?;

        // A query matches a message and the list shows conversations. The
        // first hit for a conversation is its best one — the list arrives
        // ranked — so that is the hit the row's snippet comes from.
        let mut seen = std::collections::HashSet::new();
        let mut kept: Vec<(i64, &Hit)> = Vec::new();
        for hit in &hits {
            let tid = self.thread_of(hit.id)?.unwrap_or(-hit.id);
            if seen.insert(tid) {
                kept.push((tid, hit));
            }
        }
        kept.truncate(limit as usize);
        if kept.is_empty() {
            return Ok(Vec::new());
        }

        let order: Vec<i64> = kept.iter().map(|(tid, _)| *tid).collect();
        let mut why = self.why_matched(&kept, &asked)?;
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

    /// Every statement's hits, as one list, and the words each was asked
    /// with.
    ///
    /// A query is one statement unless an `OR` has words on one side and a
    /// condition on the other (`bundles` says why). Each is asked in the
    /// shape a query without `OR` has always had, so nearly every query
    /// costs what it did, and the account and the conditions stay inside the
    /// ranking where they were put on purpose.
    ///
    /// Then the lists are put together. What matched words comes first, best
    /// match first, because that is the one order only a search can offer.
    /// What merely met conditions follows, newest first, the way a listing
    /// is: words rank, conditions filter, and with no words there is nothing
    /// for BM25 to score.
    fn hits_meeting(
        &self,
        q: &crate::search_query::SearchQuery,
        limit: u32,
        account: i64,
    ) -> Result<(Vec<Hit>, Vec<Asked>)> {
        let mut asked: Vec<Asked> = Vec::new();
        let mut ranked: Vec<(f64, Hit)> = Vec::new();
        let mut dated: Vec<Hit> = Vec::new();
        let Some(root) = &q.root else {
            return Ok((Vec::new(), asked));
        };
        let node = without_nots(root, false);
        let typing = typing_word(&node);
        // Too wide to ask a statement at a time: asked whole, as lookups.
        // Those hits are listed by date and carry no snippet, which is the
        // price of being right about a query this size.
        let statements = bundles(&node).unwrap_or_else(|| {
            vec![Bundle {
                conds: pruned(as_lookups(&node)).into_iter().collect(),
                ..Bundle::default()
            }]
        });
        for bundle in statements {
            let Some(mut bound) = self.bind(&bundle, account, typing)? else {
                continue;
            };
            match bound.asked.take() {
                Some(words) => {
                    let from = Some(asked.len());
                    for (rank, id) in self.ranked_meeting(&words, bound, limit)? {
                        ranked.push((
                            rank,
                            Hit {
                                id,
                                date_ms: 0,
                                from,
                            },
                        ));
                    }
                    asked.push(words);
                }
                None => dated.extend(self.messages_meeting(bound, limit)?),
            }
        }
        // bm25 is lower-is-better. Both sorts are stable, so one alternative
        // alone keeps exactly the order its own statement gave it.
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        dated.sort_by_key(|hit| std::cmp::Reverse(hit.date_ms));
        let mut seen = std::collections::HashSet::new();
        let hits = ranked
            .into_iter()
            .map(|(_, hit)| hit)
            .chain(dated)
            .filter(|hit| seen.insert(hit.id))
            .collect();
        Ok((hits, asked))
    }

    /// Why each kept conversation matched: the text around the hit, marked.
    ///
    /// For the rows that will be shown and no others, and in one pass per
    /// alternative. It used to be one lookup per hit, `MATCH ? AND rowid = ?`,
    /// for every hit the ranking returned, on the theory that a match against
    /// one row is a lookup and not a scan. It is not, when the last word is a
    /// prefix the prefix indexes do not cover: FTS5 gathers every term that
    /// begins with it, all over again, for each row asked about. Six hundred
    /// lookups of a common word came to 300ms at twenty thousand messages and
    /// 1.6s at a hundred thousand, against 16ms for the ranking itself — and
    /// two thirds of those snippets were for hits the conversation rollup
    /// then threw away.
    ///
    /// `+rowid` keeps the planner from turning the list back into a lookup
    /// per id: the match is walked once, and the snippet is only computed
    /// for the rows the list lets through.
    fn why_matched(
        &self,
        kept: &[(i64, &Hit)],
        asked: &[Asked],
    ) -> Result<std::collections::HashMap<i64, String>> {
        let mut why = std::collections::HashMap::new();
        for (index, words) in asked.iter().enumerate() {
            let of_these: Vec<(i64, i64)> = kept
                .iter()
                .filter(|(_, hit)| hit.from == Some(index))
                .map(|(tid, hit)| (hit.id, *tid))
                .collect();
            if of_these.is_empty() {
                continue;
            }
            if words.cjk {
                // Snippets come from the original text, never from the
                // space-separated index copy, and are marked in Rust.
                let mut body = self
                    .conn
                    .prepare_cached("SELECT body_text FROM fts_content WHERE message_id = ?1")?;
                for (id, tid) in of_these {
                    // The index row is the one that can be missing, and then
                    // the hit simply has no snippet rather than failing the
                    // search.
                    if let Some(text) = body
                        .query_row(params![id], |r| r.get::<_, String>(0))
                        .optional()?
                    {
                        why.insert(tid, cjk_snippet(&text, &words.marks));
                    }
                }
                continue;
            }
            let thread_of: std::collections::HashMap<i64, i64> = of_these.iter().copied().collect();
            let sql = format!(
                "SELECT rowid, snippet(fts_messages, 1, char(57344), char(57345), '…', 12)
                 FROM fts_messages
                 WHERE fts_messages MATCH ? AND +rowid IN ({})",
                vec!["?"; of_these.len()].join(",")
            );
            let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(words.expr.clone())];
            values.extend(
                of_these
                    .iter()
                    .map(|(id, _)| Box::new(*id) as Box<dyn rusqlite::ToSql>),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, snippet) = row?;
                if let Some(tid) = thread_of.get(&id)
                    && !snippet.is_empty()
                {
                    why.insert(*tid, snippet);
                }
            }
        }
        Ok(why)
    }

    /// Midnight at the start of a calendar day where the reader is, in
    /// milliseconds. 07 §5.1: `date:2026-08-14` means that day in local time,
    /// not in UTC.
    ///
    /// SQLite does the conversion because it is the one thing in the engine
    /// that already knows the time zone; the engine has no calendar library
    /// and this is no reason to add one. The answer is fetched before the
    /// search statement is built rather than computed inside it: `utc` makes
    /// strftime non-deterministic, so SQLite would work it out again for
    /// every row, and a plain number lets the date index do its job.
    fn local_midnight_ms(&self, day: crate::search_query::Day) -> Result<i64> {
        let text = format!("{:04}-{:02}-{:02} 00:00:00", day.year, day.month, day.day);
        let seconds: Option<i64> = self.conn.query_row(
            "SELECT CAST(strftime('%s', ?1, 'utc') AS INTEGER)",
            params![text],
            |r| r.get(0),
        )?;
        Ok(seconds.map_or_else(|| day.utc_ms(), |s| s * 1000))
    }

    /// The calendar day an instant falls on where the reader is — the other
    /// direction from the one `date:` needs, for whatever has to turn "today"
    /// or "last month" into dates the grammar can hold.
    pub fn local_day(&self, at_ms: i64) -> Result<crate::search_query::Day> {
        let text: String = self.conn.query_row(
            "SELECT strftime('%Y-%m-%d', ?1 / 1000, 'unixepoch', 'localtime')",
            params![at_ms],
            |r| r.get(0),
        )?;
        let mut parts = text.split('-').map(str::parse::<u16>);
        match (parts.next(), parts.next(), parts.next()) {
            (Some(Ok(year)), Some(Ok(month)), Some(Ok(day))) => Ok(crate::search_query::Day {
                year,
                month: month as u8,
                day: day as u8,
            }),
            _ => Err(StoreError::Rejected(
                "that instant is not a date SQLite can name".into(),
            )),
        }
    }

    /// The best `limit` matches for the words that also meet the conditions,
    /// in rank order, each with its rank.
    ///
    /// The ranking and the filter are one statement: the join puts the
    /// account and the conditions where the planner sees them, so a match
    /// in the other account is never ranked, fetched or discarded. Filtering
    /// after the ranking was tried: it either took a fixed slice, which a
    /// common word in the other account could fill entirely, or walked the
    /// ranking a page at a time, which re-ranked every match per page and
    /// held the store for seconds on a two-letter query.
    ///
    /// CROSS JOIN is SQLite's way of fixing the order: the match is walked,
    /// and each match looks its message up. Left to choose, the planner turns
    /// the join round as soon as a condition can use an index — a date range
    /// does — and asks the index about every message in the range instead,
    /// one `MATCH ? AND rowid = ?` at a time. FTS5 prices that probe at almost
    /// nothing. It is not: for a last word still being typed it gathers every
    /// term that begins with it, per probe. `after:2020-09-14 before:2020-09-17
    /// meeting` took 4.7s at twenty thousand messages that way, and 13ms this
    /// way. Walking the match is never pathological, because its worst case
    /// is the ranking a query with no conditions already pays for.
    ///
    /// Ids and nothing else. Snippets are the expensive half of a hit — a
    /// sorter has to build every row it orders, so a snippet in the ranking
    /// query is computed for every match and then thrown away for all but
    /// the first page — and they are made afterwards, in `why_matched`, for
    /// the rows that survive the rollup into conversations.
    fn ranked_meeting(&self, words: &Asked, bound: Bound, limit: u32) -> Result<Vec<(f64, i64)>> {
        let conds = bound.conditions;
        let sql = if words.cjk {
            format!(
                "SELECT f.rowid, bm25(fts_cjk, 4.0, 1.0) AS r
                 FROM fts_cjk f
                 CROSS JOIN messages m ON m.id = f.rowid
                 WHERE fts_cjk MATCH ? AND m.deleted_at_ms IS NULL{conds}
                 ORDER BY r
                 LIMIT ?"
            )
        } else {
            format!(
                "SELECT f.rowid, bm25(fts_messages, 4.0, 1.0, 2.0, 2.0) AS r
                 FROM fts_messages f
                 CROSS JOIN messages m ON m.id = f.rowid
                 WHERE fts_messages MATCH ? AND m.deleted_at_ms IS NULL{conds}
                 ORDER BY r
                 LIMIT ?"
            )
        };
        let mut all: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(words.expr.clone())];
        all.extend(bound.args);
        all.push(Box::new(limit));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(all), |r| {
            Ok((r.get::<_, f64>(1)?, r.get::<_, i64>(0)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Every message meeting the conditions, newest first — the answer when
    /// an alternative named conditions but no words. Nothing was searched
    /// for, so there is nothing to mark, and these hits carry no snippet.
    fn messages_meeting(&self, bound: Bound, limit: u32) -> Result<Vec<Hit>> {
        let conds = bound.conditions;
        let mut args = bound.args;
        let sql = format!(
            "SELECT m.id, m.date_ms
             FROM messages m
             WHERE m.deleted_at_ms IS NULL{conds}
             ORDER BY m.date_ms DESC LIMIT ?"
        );
        args.push(Box::new(limit));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok(Hit {
                id: r.get(0)?,
                date_ms: r.get(1)?,
                from: None,
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
