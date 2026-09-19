//! Search: the grammar's conditions bound into SQL, FTS hits resolved to
//! conversations, and the account wall on all of it.
//!
//! Moved verbatim from mod.rs (Phase 1.5); the free helpers it leans on —
//! match_expr, the CJK machinery, in_inbox — remain in mod.rs and arrive
//! through `use super::*`.
use super::*;
use crate::search_query::{Expr, MAX_ALTERNATIVES, State, Term, Text};

/// A term as the query finally asks for it, with every NOT around it applied.
#[derive(Clone, Copy)]
struct Lit<'a> {
    negated: bool,
    term: &'a Term,
}

/// The query with its NOTs pushed down to the terms, so that what is left is
/// only "all of these" and "any of these".
enum Node<'a> {
    Lit(Lit<'a>),
    All(Vec<Node<'a>>),
    Any(Vec<Node<'a>>),
}

fn without_nots<'a>(expr: &'a Expr, flip: bool) -> Node<'a> {
    match expr {
        Expr::Clause(clause) => Node::Lit(Lit {
            negated: clause.negated != flip,
            term: &clause.term,
        }),
        Expr::Not(inner) => without_nots(inner, !flip),
        // Not (a and b) is (not a) or (not b), and the other way about.
        Expr::All(parts) | Expr::Any(parts) => {
            let parts = parts.iter().map(|p| without_nots(p, flip)).collect();
            if matches!(expr, Expr::All(_)) != flip {
                Node::All(parts)
            } else {
                Node::Any(parts)
            }
        }
    }
}

/// Words to find, as a tree FTS5 can be asked in one expression.
#[derive(Clone)]
enum Words<'a> {
    Leaf(Option<&'static str>, &'a Text),
    All(Vec<Words<'a>>),
    Any(Vec<Words<'a>>),
}

/// Conditions, as a tree SQL can be asked in one predicate.
#[derive(Clone)]
enum Cond<'a> {
    Lit(Lit<'a>),
    /// Words that must not be there, looked up on their own and taken away.
    Without(Option<&'static str>, &'a Text),
    All(Vec<Cond<'a>>),
    Any(Vec<Cond<'a>>),
}

/// The words a term looks for, and the column it looks in.
fn text_of(term: &Term) -> Option<(Option<&'static str>, &Text)> {
    match term {
        Term::Text(text) => Some((None, text)),
        Term::Subject(text) => Some((Some("subject"), text)),
        _ => None,
    }
}

impl Words<'_> {
    /// Whether these words are asked of the per-character CJK index. A
    /// message with a CJK word in it is in that index with all of its words,
    /// so words that all have to be there can be asked of it together.
    fn cjk(&self) -> bool {
        match self {
            Words::Leaf(_, text) => has_cjk(&text.value),
            Words::All(parts) | Words::Any(parts) => parts.iter().any(Words::cjk),
        }
    }

    fn leaves<'a>(&'a self, out: &mut Vec<&'a Text>) {
        match self {
            Words::Leaf(_, text) => out.push(text),
            Words::All(parts) | Words::Any(parts) => parts.iter().for_each(|p| p.leaves(out)),
        }
    }
}

/// The node as words alone, if that is all it is.
///
/// Alternatives have to share an index to share an expression: `東京 OR
/// tokyo` asked of the CJK index would miss every message that says "tokyo"
/// and has no CJK in it, because those are not in that index at all.
fn as_words<'a>(node: &Node<'a>) -> Option<Words<'a>> {
    match node {
        Node::Lit(lit) if !lit.negated => {
            text_of(lit.term).map(|(column, text)| Words::Leaf(column, text))
        }
        Node::Lit(_) => None,
        Node::All(parts) => parts
            .iter()
            .map(as_words)
            .collect::<Option<_>>()
            .map(Words::All),
        Node::Any(parts) => {
            let parts: Vec<Words> = parts.iter().map(as_words).collect::<Option<_>>()?;
            let cjk = parts.first()?.cjk();
            parts
                .iter()
                .all(|p| p.cjk() == cjk)
                .then_some(Words::Any(parts))
        }
    }
}

/// The node as conditions alone, if that is all it is. Words that must not
/// be there count: they are a lookup and a subtraction, which SQL can do
/// anywhere. Words that must be there do not, because they are what ranks.
fn as_cond<'a>(node: &Node<'a>) -> Option<Cond<'a>> {
    match node {
        Node::Lit(lit) => match text_of(lit.term) {
            None => Some(Cond::Lit(*lit)),
            Some((column, text)) if lit.negated => Some(Cond::Without(column, text)),
            Some(_) => None,
        },
        Node::All(parts) => parts
            .iter()
            .map(as_cond)
            .collect::<Option<_>>()
            .map(Cond::All),
        Node::Any(parts) => parts
            .iter()
            .map(as_cond)
            .collect::<Option<_>>()
            .map(Cond::Any),
    }
}

/// One statement's worth of a query: words to rank by, words to leave out,
/// and conditions, all of which have to hold.
#[derive(Default, Clone)]
struct Bundle<'a> {
    wanted: Vec<Words<'a>>,
    unwanted: Vec<(Option<&'static str>, &'a Text)>,
    conds: Vec<Cond<'a>>,
}

/// The query as statements, any of which may match.
///
/// Like is kept with like. Words joined by `OR` are one FTS5 expression and
/// conditions joined by `OR` are one SQL predicate, so `(from:sam OR
/// from:dana) (invoice OR receipt)` is a single statement and a single
/// ranking, and costs what `from:sam invoice` costs. Only an `OR` with words
/// on one side and a condition on the other has to be asked once per side —
/// a ranking has nothing to say about a message that matched no words — and
/// those are multiplied out, up to the limit.
fn bundles<'a>(node: &Node<'a>) -> Vec<Bundle<'a>> {
    if let Some(words) = as_words(node) {
        return vec![Bundle {
            wanted: vec![words],
            ..Bundle::default()
        }];
    }
    match node {
        Node::Lit(lit) => vec![match text_of(lit.term) {
            Some(unwanted) => Bundle {
                unwanted: vec![unwanted],
                ..Bundle::default()
            },
            None => Bundle {
                conds: vec![Cond::Lit(*lit)],
                ..Bundle::default()
            },
        }],
        Node::All(parts) => {
            let mut out = vec![Bundle::default()];
            for part in parts {
                let options = bundles(part);
                out = out
                    .iter()
                    .flat_map(|left| {
                        options.iter().map(move |right| {
                            let mut both = left.clone();
                            both.wanted.extend(right.wanted.iter().cloned());
                            both.unwanted.extend(right.unwanted.iter().copied());
                            both.conds.extend(right.conds.iter().cloned());
                            both
                        })
                    })
                    .take(MAX_ALTERNATIVES)
                    .collect();
            }
            out
        }
        Node::Any(parts) => {
            let (mut latin, mut cjk, mut conds, mut rest) = (vec![], vec![], vec![], vec![]);
            for part in parts {
                if let Some(words) = as_words(part) {
                    if words.cjk() { &mut cjk } else { &mut latin }.push(words);
                } else if let Some(cond) = as_cond(part) {
                    conds.push(cond);
                } else {
                    rest.extend(bundles(part));
                }
            }
            let mut out = Vec::new();
            for mut words in [latin, cjk] {
                let words = match words.len() {
                    0 => continue,
                    1 => words.remove(0),
                    _ => Words::Any(words),
                };
                out.push(Bundle {
                    wanted: vec![words],
                    ..Bundle::default()
                });
            }
            if !conds.is_empty() {
                out.push(Bundle {
                    conds: vec![if conds.len() == 1 {
                        conds.remove(0)
                    } else {
                        Cond::Any(conds)
                    }],
                    ..Bundle::default()
                });
            }
            out.extend(rest);
            out.truncate(MAX_ALTERNATIVES);
            out
        }
    }
}

/// A word with no letter or digit in it is nothing the tokenizer keeps.
fn indexable(text: &Text) -> bool {
    text.value.chars().any(char::is_alphanumeric)
}

/// One term as FTS5 has to be asked for it: a quoted phrase, exactly as
/// `match_expr` builds them, so that what was typed never reaches MATCH bare.
fn phrase(column: Option<&str>, text: &Text, for_cjk: bool, prefix: bool) -> String {
    let inner = if for_cjk {
        let parts = cjk_parts(&text.value);
        if parts.len() == 1 {
            parts[0].clone()
        } else {
            format!("({})", parts.join(" AND "))
        }
    } else if prefix && last_alnum_run(&text.value).chars().count() >= 2 {
        format!("{}*", quote_token(&text.value))
    } else {
        quote_token(&text.value)
    };
    match column {
        Some(column) => format!("{column} : {inner}"),
        None => inner,
    }
}

/// Words as one FTS5 expression, or `None` when none of them is a word the
/// index keeps. Every AND and OR is written out: FTS5 will not take an
/// implicit AND beside a bracket.
fn fts(words: &Words, for_cjk: bool, typing: Option<&Text>) -> Option<String> {
    match words {
        Words::Leaf(_, text) if !indexable(text) => None,
        Words::Leaf(column, text) => {
            let prefix = typing.is_some_and(|last| std::ptr::eq(last, *text));
            Some(phrase(*column, text, for_cjk, prefix))
        }
        Words::All(parts) | Words::Any(parts) => {
            let asked: Vec<String> = parts
                .iter()
                .filter_map(|p| fts(p, for_cjk, typing))
                .collect();
            let joiner = if matches!(words, Words::All(_)) {
                " AND "
            } else {
                " OR "
            };
            match asked.len() {
                0 => None,
                1 => asked.into_iter().next(),
                _ => Some(format!("({})", asked.join(joiner))),
            }
        }
    }
}

/// Whether a condition asks, un-negated, for something a term names.
fn asks(cond: &Cond, wanted: &dyn Fn(&Term) -> bool) -> bool {
    match cond {
        Cond::Lit(lit) => !lit.negated && wanted(lit.term),
        Cond::Without(..) => false,
        Cond::All(parts) | Cond::Any(parts) => parts.iter().any(|p| asks(p, wanted)),
    }
}

fn is_the_bin(term: &Term) -> bool {
    matches!(term, Term::In(role) if role == "spam" || role == "trash")
}

/// One alternative of a query, bound into SQL.
struct Bound {
    /// The words to rank by, when there are any.
    asked: Option<Asked>,
    /// The conditions, each beginning ` AND`, and the values they bind.
    conditions: String,
    args: Vec<Box<dyn rusqlite::ToSql>>,
}

/// The words one alternative asked for, as the index has to be asked.
struct Asked {
    /// The FTS5 expression.
    expr: String,
    /// Whether it is asked of the per-character CJK index.
    cjk: bool,
    /// The words a CJK snippet should mark. FTS5 marks its own.
    marks: String,
}

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
        for bundle in bundles(&without_nots(root, false)) {
            let Some(mut bound) = self.bind(&bundle, account)? else {
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

    /// One statement's worth of a query, bound into SQL — or `None` when it
    /// asks for nothing, or for something nothing can match.
    ///
    /// Built rather than interpolated: `from:` and `in:` carry whatever was
    /// typed, and a search box that reaches SQL is the oldest mistake there
    /// is. Every value is a bound parameter; every word that reaches MATCH is
    /// a quoted phrase. The only text written into a statement here is text
    /// this file wrote.
    fn bind(&self, bundle: &Bundle, account: i64) -> Result<Option<Bound>> {
        // The words first, because they decide which index is asked.
        //
        // A word with no letter or digit in it is simply skipped among other
        // words. On its own it finds nothing, which is what a half-typed
        // `[acme]` should do: for the length of one keystroke the query is
        // `in:inbox [`, and that must not list the whole inbox.
        let mut leaves = Vec::new();
        bundle.wanted.iter().for_each(|w| w.leaves(&mut leaves));
        let cjk = bundle.wanted.iter().any(Words::cjk);
        // As-you-type: the last word is a prefix while it is still being
        // typed. Last among the words, not last in the field — a chip writes
        // its token after them, and clicking one mid-word must not empty the
        // list. Never a quoted word, which somebody finished; never one in
        // the CJK index, where a character is already a whole token.
        let typing = leaves.last().copied().filter(|text| !text.exact);
        let wanted: Vec<String> = bundle
            .wanted
            .iter()
            .filter_map(|w| fts(w, cjk, typing))
            .collect();
        if wanted.is_empty() && !bundle.wanted.is_empty() {
            return Ok(None);
        }
        let mut expr = wanted.join(" AND ");

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
        // nothing else does. Asking on one side of an `OR` is not asking on
        // the other: `in:spam OR from:sam` lets spam in on the left only, and
        // `any_sql` is where a bracketed `(in:spam OR from:sam)` is held to
        // the same rule.
        let guard = not_binned("m");
        if !bundle.conds.iter().any(|c| asks(c, &is_the_bin)) {
            sql.push_str(&format!(" AND {guard}"));
        }

        // Words that must not be there. Beside the wanted ones they are FTS5's
        // own NOT, which costs nothing. Otherwise they are looked up on their
        // own and taken away: NOT needs something on its left, so `-draft`
        // with no other word cannot be a MATCH, and a CJK word has to be asked
        // of the CJK index whichever index the wanted words are in.
        let mut refused: Vec<String> = Vec::new();
        let mut excluded = 0;
        for (column, text) in bundle.unwanted.iter().filter(|(_, t)| indexable(t)) {
            excluded += 1;
            if !wanted.is_empty() && (cjk || !has_cjk(&text.value)) {
                refused.push(phrase(*column, text, cjk, false));
            } else {
                sql.push_str(&format!(
                    " AND {}",
                    Self::without_sql(*column, text, &mut args)
                ));
            }
        }
        if !refused.is_empty() {
            expr = format!("({expr}) NOT ({})", refused.join(" OR "));
        }

        let snoozed = bundle
            .conds
            .iter()
            .any(|c| asks(c, &|term| *term == Term::Is(State::Snoozed)));
        for cond in &bundle.conds {
            let predicate = self.cond_sql(cond, snoozed, &guard, &mut args)?;
            sql.push_str(&format!(" AND {predicate}"));
        }

        // Every word in it was punctuation and every one was excluded:
        // nothing is left to ask, and that is not the same as everything.
        if wanted.is_empty() && excluded == 0 && bundle.conds.is_empty() {
            return Ok(None);
        }
        Ok(Some(Bound {
            asked: (!wanted.is_empty()).then(|| Asked {
                expr,
                cjk,
                marks: leaves
                    .iter()
                    .find(|text| has_cjk(&text.value))
                    .map(|text| text.value.clone())
                    .unwrap_or_default(),
            }),
            conditions: sql,
            args,
        }))
    }

    /// Words that must not be there, as a lookup of their own in whichever
    /// index holds their script, taken away from the rest.
    fn without_sql(
        column: Option<&str>,
        text: &Text,
        args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) -> String {
        let cjk = has_cjk(&text.value);
        let index = if cjk { "fts_cjk" } else { "fts_messages" };
        args.push(Box::new(phrase(column, text, cjk, false)));
        format!("m.id NOT IN (SELECT rowid FROM {index} WHERE {index} MATCH ?)")
    }

    /// A tree of conditions as one SQL predicate, its values bound in the
    /// order it writes them.
    fn cond_sql(
        &self,
        cond: &Cond,
        snoozed: bool,
        guard: &str,
        args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) -> Result<String> {
        Ok(match cond {
            Cond::Lit(lit) if lit.negated => {
                format!("NOT ({})", self.predicate(lit.term, snoozed, args)?)
            }
            Cond::Lit(lit) => self.predicate(lit.term, snoozed, args)?,
            Cond::Without(column, text) if indexable(text) => {
                Self::without_sql(*column, text, args)
            }
            // Punctuation that must not be there: true of every message.
            Cond::Without(..) => "1".to_string(),
            Cond::All(parts) => {
                let parts = parts
                    .iter()
                    .map(|p| self.cond_sql(p, snoozed, guard, args))
                    .collect::<Result<Vec<_>>>()?;
                format!("({})", parts.join(" AND "))
            }
            Cond::Any(parts) => {
                // The bin is let in one alternative at a time, in brackets as
                // out of them: the side that named it, and no other.
                let named = parts.iter().any(|p| asks(p, &is_the_bin));
                let parts = parts
                    .iter()
                    .map(|p| {
                        let sql = self.cond_sql(p, snoozed, guard, args)?;
                        Ok(if named && !asks(p, &is_the_bin) {
                            format!("({sql} AND {guard})")
                        } else {
                            sql
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                format!("({})", parts.join(" OR "))
            }
        })
    }

    /// One condition as SQL, its values bound as it goes.
    fn predicate(
        &self,
        term: &Term,
        snoozed: bool,
        args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) -> Result<String> {
        // SQLite's `lower` folds ASCII only. A value that is not ASCII goes
        // through ours; an ASCII one can only ever match ASCII letters, so it
        // keeps the built-in, which is several times cheaper per row.
        let fold = |value: &str| {
            if value.is_ascii() {
                "lower"
            } else {
                "petrel_lower"
            }
        };
        let contains = |value: &str| format!("%{}%", folders::like_escape(&value.to_lowercase()));
        Ok(match term {
            // Words are asked of the index, never here.
            Term::Text(_) | Term::Subject(_) => "1".to_string(),
            Term::From(who) => {
                for _ in 0..2 {
                    args.push(Box::new(contains(who)));
                }
                let lower = fold(who);
                format!(
                    "({lower}(coalesce(m.from_addr,'')) LIKE ? ESCAPE '\\'
                      OR {lower}(coalesce(m.from_display,'')) LIKE ? ESCAPE '\\')"
                )
            }
            Term::To(who) | Term::Cc(who) => {
                let role = if matches!(term, Term::To(_)) {
                    "to"
                } else {
                    "cc"
                };
                for _ in 0..2 {
                    args.push(Box::new(contains(who)));
                }
                // `addr_norm` was lowercased on the way in, by Rust.
                let lower = fold(who);
                format!(
                    "EXISTS (SELECT 1 FROM message_addresses a
                             WHERE a.message_id = m.id AND a.role = '{role}'
                               AND (a.addr_norm LIKE ? ESCAPE '\\'
                                    OR {lower}(coalesce(a.display,'')) LIKE ? ESCAPE '\\'))"
                )
            }
            Term::In(name) => {
                // A role, or a folder the user made — by full path or by leaf, so
                // `in:receipts` and `in:projects/petrel` both say what they mean.
                // The parser lowercased the value; the comparisons follow suit.
                // EXISTS rather than `m.id IN (…)`. The IN form was tried, on the
                // theory that the correlated subquery made the planner walk the
                // mailbox; measured on a real store of twenty-nine thousand it was
                // slower at every width — 7.4ms against 1.3ms for one exact token,
                // 92ms against 84ms for the broadest prefix — because it builds the
                // whole mailbox's placement list whatever the match narrows to.
                // Both plans open on the FTS match, so neither walks the mailbox.
                for _ in 0..2 {
                    args.push(Box::new(name.clone()));
                }
                for _ in 0..2 {
                    args.push(Box::new(folders::like_escape(name)));
                }
                let lower = fold(name);
                let placed = format!(
                    "EXISTS (SELECT 1 FROM placements p JOIN folders f ON f.id = p.folder_id
                             WHERE p.message_id = m.id
                               AND (f.role = ?
                                    OR {lower}(f.path) = ?
                                    OR {lower}(f.path) LIKE '%/' || ? ESCAPE '\\'
                                    OR {lower}(f.path) LIKE '%.' || ? ESCAPE '\\'))"
                );
                // Snoozing takes a message out of the inbox until it comes back.
                // That is what the Inbox view's predicate says and what its unread
                // badge counts, and search used to disagree: `in:inbox is:unread`
                // returned the snoozed ones too, so the list and the number beside
                // the mailbox differed by exactly the mail somebody had put off.
                //
                // Only the inbox, because that is the only view snoozing hides
                // from — and not when `is:snoozed` asked for them by name, since
                // snoozing hides mail rather than burying it.
                if name == "inbox" && !snoozed {
                    format!(
                        "({placed} AND coalesce(m.snoozed_until_ms, 0)
                                       <= (strftime('%s','now') * 1000))"
                    )
                } else {
                    placed
                }
            }
            Term::Tag(name) => {
                args.push(Box::new(name.to_lowercase()));
                let lower = fold(name);
                format!(
                    "EXISTS (SELECT 1 FROM message_tags mt JOIN tags tg ON tg.id = mt.tag_id
                             WHERE mt.message_id = m.id AND {lower}(tg.name) = ?)"
                )
            }
            Term::Filename(part) => {
                args.push(Box::new(contains(part)));
                let lower = fold(part);
                format!(
                    "EXISTS (SELECT 1 FROM attachments a
                             WHERE a.message_id = m.id
                               AND {lower}(coalesce(a.filename,'')) LIKE ? ESCAPE '\\')"
                )
            }
            Term::HasAttachment => "m.has_attachments = 1".to_string(),
            Term::Is(State::Unread) => format!("m.flags & {} = 0", flags::SEEN),
            Term::Is(State::Read) => format!("m.flags & {} != 0", flags::SEEN),
            Term::Is(State::Starred) => format!("m.flags & {} != 0", flags::FLAGGED),
            Term::Is(State::Snoozed) => {
                "coalesce(m.snoozed_until_ms, 0) > (strftime('%s','now') * 1000)".to_string()
            }
            // `after:` takes in the day it names and `before:` leaves it
            // out, so `after:2026-08-01 before:2026-09-01` is August and
            // nothing else. The year chip has always written `after:2026`
            // for "this year"; read the other way round it would mean 2027.
            Term::After(when) => {
                args.push(Box::new(self.local_midnight_ms(when.first_day())?));
                "m.date_ms >= ?".to_string()
            }
            Term::Before(when) => {
                args.push(Box::new(self.local_midnight_ms(when.first_day())?));
                "m.date_ms < ?".to_string()
            }
            Term::On(when) => {
                args.push(Box::new(self.local_midnight_ms(when.first_day())?));
                args.push(Box::new(self.local_midnight_ms(when.day_after())?));
                "(m.date_ms >= ? AND m.date_ms < ?)".to_string()
            }
        })
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
