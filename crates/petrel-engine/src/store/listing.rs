//! The conversation list: rows, one conversation by id, its messages, and
//! the counts the rail wears.
//!
//! Moved verbatim from mod.rs (Phase 1.5). The listing SQL and the counts
//! share the ListView predicates, which stay in mod.rs with the enum.
use super::*;
use rusqlite::OptionalExtension;
use std::collections::HashMap;

/// Whether a row comes strictly after a cursor in a sender or subject sort.
///
/// The order the query states: conversations with nothing in the field last
/// whichever way the sort runs, then the field itself in the sort's
/// direction, then newest first within a tie.
fn sorts_after(value: &str, date_ms: i64, cursor: &str, cursor_ms: i64, ascending: bool) -> bool {
    if value.is_empty() != cursor.is_empty() {
        return value.is_empty();
    }
    if value != cursor {
        return if ascending {
            value > cursor
        } else {
            value < cursor
        };
    }
    date_ms < cursor_ms
}

/// The tags a row wears, as one JSON array per conversation.
///
/// Shared by the two queries that build a row — the list's and the one search
/// uses to fetch its hits by id — because they had been written out twice and
/// drifted: the second lost `id` from the object. `ThreadRowTag` needs all
/// three fields, and the parse helper turns a failed parse into an empty list,
/// so the loss was silent and total. Every search result came back untagged.
const TAGS_JSON: &str = "(SELECT json_group_array(
                         json_object('id', tg.id, 'name', tg.name, 'colour', coalesce(tg.colour,'')))
                     FROM (SELECT DISTINCT mt.tag_id FROM message_tags mt
                           JOIN messages mm ON mm.id = mt.message_id
                           WHERE coalesce(mm.thread_id, -mm.id) = t.thread_id) d
                     JOIN tags tg ON tg.id = d.tag_id) AS tags_json";

type ThreadDetailRow = (
    i64,
    String,
    String,
    String,
    String,
    i64,
    i64,
    bool,
    Option<String>,
    Option<String>,
);

/// SQLite allows 999 bound variables. Stay well under that for the
/// address and attachment `IN` lists on a fat hydrate.
const HYDRATE_IN_CHUNK: usize = 400;

fn thread_detail_row(r: &rusqlite::Row) -> rusqlite::Result<ThreadDetailRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
    ))
}

/// The wire Message-ID behind a stored dedupe key, if the key is one.
///
/// The key is the header when the message had one, and stands in for it
/// otherwise: a blob hash for a message with no Message-ID, and a `::copy-N`
/// suffix on a second server copy of the same message. A reply must name
/// only the real thing — an invented id threads with nothing, and a
/// suffixed one threads with nothing either.
fn wire_msgid(key: Option<String>) -> Option<String> {
    let key = key?;
    if key.is_empty() || key.starts_with("blake3:") {
        return None;
    }
    let bare = match key.find("::copy-") {
        Some(at) => &key[..at],
        None => key.as_str(),
    };
    (!bare.is_empty()).then(|| bare.to_string())
}

/// Display name and addr_norm, in header order, split by role.
#[derive(Clone, Default)]
struct MessageAddrs {
    to: Vec<(String, String)>,
    cc: Vec<(String, String)>,
}

fn sql_in_marks(n: usize) -> String {
    let mut s = String::with_capacity(n.saturating_mul(2));
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s
}

impl Store {
    pub fn list_threads(
        &self,
        view: &ListView,
        offset: u32,
        limit: u32,
        sort: Sort,
    ) -> Result<Vec<ThreadListing>> {
        // Scoped to the account on screen. The query was written when one
        // account was all there was; with two, every view showed both
        // mailboxes merged — which is exactly the send-from-the-wrong-address
        // mistake that "one active at a time" exists to prevent. A missing
        // account (an empty store) scopes to nothing, which lists nothing.
        let account = self.active_account()?.unwrap_or(-1);
        self.list_threads_for(account, view, offset, limit, sort)
    }

    /// `list_threads` for a named account rather than the active one — for
    /// callers like export that act on a mailbox the window is not showing.
    pub fn list_threads_for(
        &self,
        account: i64,
        view: &ListView,
        offset: u32,
        limit: u32,
        sort: Sort,
    ) -> Result<Vec<ThreadListing>> {
        self.listing_rows(
            account,
            ListingQuery {
                inner: &view.predicate("messages"),
                outer: &view.predicate("m"),
                limit,
                offset,
                bound: view.bound().map(str::to_string),
                per_message: matches!(view, ListView::Folder(r) if r == "drafts"),
                sort,
                before: None,
            },
        )
    }

    /// The next page after a conversation the caller already has.
    ///
    /// Offset would mean "skip N", which shifts when new mail lands at the
    /// top. Naming the last row keeps the rest of the list stable. The walk
    /// still starts from the newest message — filtering by date would let an
    /// older message of an already-listed thread look like a new row — and
    /// stops one page after the named conversation.
    pub fn list_threads_after(
        &self,
        view: &ListView,
        limit: u32,
        sort: Sort,
        before_date_ms: i64,
        before_thread_id: i64,
    ) -> Result<Vec<ThreadListing>> {
        let account = self.active_account()?.unwrap_or(-1);
        self.listing_rows(
            account,
            ListingQuery {
                inner: &view.predicate("messages"),
                outer: &view.predicate("m"),
                limit,
                offset: 0,
                bound: view.bound().map(str::to_string),
                per_message: matches!(view, ListView::Folder(r) if r == "drafts"),
                sort,
                before: Some((before_date_ms, before_thread_id)),
            },
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
        let account = self.active_account()?.unwrap_or(-1);
        let rows = self.listing_rows(
            account,
            ListingQuery {
                inner: "coalesce(thread_id, -id) = cast(?3 AS INTEGER)",
                outer: "coalesce(m.thread_id, -m.id) = cast(?3 AS INTEGER)",
                limit: 1,
                offset: 0,
                bound: Some(thread_id.to_string()),
                per_message: false,
                sort: Sort::default(),
                before: None,
            },
        )?;
        Ok(rows.into_iter().next())
    }
}

/// One listing's shape: the predicates, the page, and how rows group.
struct ListingQuery<'a> {
    inner: &'a str,
    outer: &'a str,
    limit: u32,
    offset: u32,
    bound: Option<String>,
    /// Drafts list one by one: a draft is a thing you finish, not a
    /// conversation, and two drafts that happen to share a subject — or even
    /// a Message-ID, edited apart on the server — are still two drafts.
    /// Every other view groups into conversations.
    per_message: bool,
    sort: Sort,
    /// Date-sort cursor: the last row's `(date_ms, thread_id)`. Sender and
    /// subject sorts ignore it and keep walking from offset, because those
    /// orders have no index to resume on.
    before: Option<(i64, i64)>,
}

impl Store {
    /// The conversation-list query, shared by the views and by a lookup of one.
    ///
    /// Two steps, and the order is the whole performance story. Aggregating
    /// first and paging afterwards means grouping every message in the
    /// mailbox — participants, counts, flags — to show a page of rows: at a
    /// hundred thousand messages that measured 562ms to open a list, against
    /// a 150ms budget, and no index helps because the work is real. So the
    /// page is chosen first, by walking newest-first down an index and
    /// collecting distinct conversations until there are enough, and only
    /// those conversations are aggregated.
    fn listing_rows(&self, account: i64, q: ListingQuery<'_>) -> Result<Vec<ThreadListing>> {
        // Borrowed, not destructured: page_keys wants the whole query and the
        // row fetch wants most of it, so moving fields out of it here means
        // neither can have it.
        let ListingQuery {
            inner,
            outer,
            bound,
            per_message,
            ..
        } = &q;
        let keys = self.page_keys(account, &q)?;
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        self.rows_for_keys(account, inner, outer, *per_message, &keys, bound.clone())
    }

    /// The conversations one page of the list shows, newest first.
    ///
    /// Walks messages down `(account_id, date_ms DESC)` and keeps the first
    /// sighting of each conversation, stopping the moment the page is full —
    /// SQLite streams the rows, so a mailbox of any size costs about a page's
    /// worth of them. A conversation's *position* is its newest message,
    /// which is exactly what walking newest-first gives.
    fn page_keys(&self, account: i64, q: &ListingQuery<'_>) -> Result<Vec<i64>> {
        let ListingQuery {
            inner,
            limit,
            offset,
            bound,
            per_message,
            sort,
            before,
            ..
        } = q;
        let (limit, offset, per_message, sort, before) =
            (*limit, *offset, *per_message, *sort, *before);
        let bound = bound.clone();
        let key = if per_message {
            "-id"
        } else {
            "coalesce(thread_id, -id)"
        };
        // Two shapes, and which one runs is the whole performance story.
        //
        // By date, the walk *is* the sort: stream messages down the index and
        // keep the first sighting of each conversation, because a
        // conversation's position by date is its newest message and that is
        // the first one met. Costs a page, whatever the mailbox holds.
        //
        // By sender or subject there is no such shortcut. Those are properties
        // of the conversation's newest message, which is not known until every
        // conversation has been resolved to one — so the set is grouped first
        // and sorted afterwards, and the whole matching set is touched. 139ms
        // on a real 26,000-message inbox, which is affordable for a sort
        // somebody chose and would not be for every list open.
        let sql = match sort.key {
            SortKey::Date if !sort.ascending => format!(
                "SELECT {key}, date_ms FROM messages
                 WHERE deleted_at_ms IS NULL AND account_id = {account} AND {inner}
                 ORDER BY date_ms DESC, {key} DESC"
            ),
            SortKey::Date => {
                // Ascending cannot stream. The first message met of a
                // conversation is then its oldest, but a conversation's date —
                // in the row, and so in the cursor — is its newest. Walking by
                // the oldest put conversations before rows they should follow
                // and left the cursor at a position the walk never reached, so
                // pages skipped conversations. Grouped first, like sender and
                // subject, at the same cost, for a sort somebody chose.
                format!(
                    "SELECT n.k, n.d FROM (
                       SELECT {key} AS k, max(date_ms) AS d FROM messages
                        WHERE deleted_at_ms IS NULL AND account_id = {account} AND {inner}
                        GROUP BY k
                     ) n
                     ORDER BY n.d ASC, n.k ASC"
                )
            }
            SortKey::Sender | SortKey::Subject => {
                let dir = if sort.ascending { "ASC" } else { "DESC" };
                // Empty last either way: a conversation with no subject sorts
                // to the end rather than to the top, where it would look like
                // the answer.
                let field = match sort.key {
                    SortKey::Sender => {
                        "lower(coalesce(nullif(n.from_display,''), n.from_addr, ''))"
                    }
                    _ => "lower(coalesce(nullif(n.subject,''), ''))",
                };
                format!(
                    "SELECT n.k, n.d, {field} AS s FROM (
                       SELECT {key} AS k,
                              max(date_ms) AS d,
                              from_display, from_addr, subject
                         FROM messages
                        WHERE deleted_at_ms IS NULL AND account_id = {account} AND {inner}
                        GROUP BY k
                     ) n
                     ORDER BY nullif({field}, '') IS NULL, {field} {dir}, n.d DESC"
                )
            }
        };
        // What the cursor conversation sorts under, read from the messages
        // themselves rather than from the page it came on. By sender or
        // subject the walk looks for the cursor *row*, and a row another
        // client had archived meanwhile was not in the walk at all — so the
        // page came back empty and the list ended, halfway down a mailbox.
        // With its value known, the page can start where the row would have
        // been.
        //
        // Through this view first, because that is the value the walk
        // compares against: a conversation's sender is its newest message's,
        // and its newest message *in the inbox* is not always its newest
        // message. Falling back to the whole account is for the case this
        // exists for — the conversation has left the view, so the view can
        // no longer say what it sorted under, and its newest message
        // anywhere is the closest thing to the answer.
        let cursor_value: Option<String> = match (sort.key, before) {
            (SortKey::Sender | SortKey::Subject, Some((_, cursor_k))) => {
                let expr = match sort.key {
                    SortKey::Sender => "lower(coalesce(nullif(from_display,''), from_addr, ''))",
                    _ => "lower(coalesce(nullif(subject,''), ''))",
                };
                // The key is an i64 the caller handed in, which is what
                // makes writing it into the SQL safe; the view's own bound
                // value is still bound, as ?3.
                let scoped = format!(
                    "SELECT {expr} FROM messages
                      WHERE deleted_at_ms IS NULL AND account_id = {account}
                        AND {key} = {cursor_k} AND {inner}
                      ORDER BY date_ms DESC LIMIT 1"
                );
                let mut stmt = self.conn.prepare_cached(&scoped)?;
                let supplied: Vec<Box<dyn rusqlite::ToSql>> = vec![
                    Box::new(rusqlite::types::Null),
                    Box::new(rusqlite::types::Null),
                    match bound.clone() {
                        Some(b) => Box::new(b) as Box<dyn rusqlite::ToSql>,
                        None => Box::new(rusqlite::types::Null),
                    },
                ];
                let wanted = stmt.parameter_count();
                let in_view: Option<String> = stmt
                    .query_row(
                        rusqlite::params_from_iter(supplied.into_iter().take(wanted)),
                        |r| r.get(0),
                    )
                    .optional()?;
                match in_view {
                    Some(v) => Some(v),
                    None => self
                        .conn
                        .query_row(
                            &format!(
                                "SELECT {expr} FROM messages
                                  WHERE deleted_at_ms IS NULL AND account_id = {account}
                                    AND {key} = ?1
                                  ORDER BY date_ms DESC LIMIT 1"
                            ),
                            params![cursor_k],
                            |r| r.get(0),
                        )
                        .optional()?,
                }
            }
            _ => None,
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        // A view's predicate binds its folder role or tag name as ?3; one
        // without a bound references no parameters at all. Bind exactly as
        // many as this statement asks for, filling the two lower slots it
        // never reads.
        let supplied: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(rusqlite::types::Null),
            Box::new(rusqlite::types::Null),
            match bound {
                Some(b) => Box::new(b) as Box<dyn rusqlite::ToSql>,
                None => Box::new(rusqlite::types::Null),
            },
        ];
        let wanted = stmt.parameter_count();
        let mut rows = stmt.query(rusqlite::params_from_iter(
            supplied.into_iter().take(wanted),
        ))?;
        // A cursor names a row the caller already has. Walking continues past
        // it rather than from a numeric offset, so new mail prepended at the
        // top does not shift every later page. The walk still starts from the
        // newest message: filtering the SQL by date would look cheaper and is
        // wrong, because an older message of an already-listed conversation
        // would masquerade as a new row. Every conversation met on the way
        // down goes into `seen`, and that is what keeps it out of the page.
        //
        // By date the cursor is a *position*, `(date_ms, key)` compared the
        // way the query sorts, and the page starts at the first row past it.
        // Looking for the cursor row itself was the version before this one,
        // and it stranded the list twice over: a conversation that gained a
        // reply had moved to the top, so "after it" was the whole mailbox
        // again and the page came back full of rows already on screen; one
        // archived by another client was not there at all, and an empty page
        // read as the end of the list. By sender or subject there is no
        // position to compare, so those still look for the row.
        let want = if before.is_some() {
            limit as usize
        } else {
            (offset as usize).saturating_add(limit as usize)
        };
        let mut seen: std::collections::HashSet<i64> = Default::default();
        let mut ordered: Vec<i64> = Vec::with_capacity(want.min(1024));
        let mut skipping = before.is_some();
        let mut found_cursor = before.is_none();
        while let Some(row) = rows.next()? {
            let k: i64 = row.get(0)?;
            let d: i64 = row.get(1)?;
            if !seen.insert(k) {
                continue;
            }
            if skipping {
                match (sort.key, before) {
                    (SortKey::Date, Some((cursor_d, cursor_k))) => {
                        let past = if sort.ascending {
                            (d, k) > (cursor_d, cursor_k)
                        } else {
                            (d, k) < (cursor_d, cursor_k)
                        };
                        if !past {
                            continue;
                        }
                        // This row is the first of the page.
                        skipping = false;
                        found_cursor = true;
                    }
                    (_, Some((cursor_d, cursor_k))) => {
                        if k == cursor_k {
                            skipping = false;
                            found_cursor = true;
                            continue;
                        }
                        // The cursor row is met before anything that sorts
                        // strictly after it, so reaching one of those means
                        // it is no longer in this view — and this row is
                        // where the page begins. Rows sorting level with it
                        // are still skipped, which is what happens while it
                        // is there.
                        let past = match cursor_value.as_deref() {
                            Some(cv) => sorts_after(
                                &row.get::<_, String>(2)?,
                                d,
                                cv,
                                cursor_d,
                                sort.ascending,
                            ),
                            None => false,
                        };
                        if !past {
                            continue;
                        }
                        skipping = false;
                        found_cursor = true;
                    }
                    (_, None) => unreachable!("skipping only with a cursor"),
                }
            }
            ordered.push(k);
            if ordered.len() >= want {
                break;
            }
        }
        if before.is_some() && !found_cursor {
            return Ok(Vec::new());
        }
        if before.is_some() {
            return Ok(ordered);
        }
        Ok(ordered.into_iter().skip(offset as usize).collect())
    }

    /// The full row for each of a known set of conversations.
    fn rows_for_keys(
        &self,
        account: i64,
        inner: &str,
        outer: &str,
        per_message: bool,
        keys: &[i64],
        bound: Option<String>,
    ) -> Result<Vec<ThreadListing>> {
        let limit = keys.len() as u32;
        let offset = 0u32;
        let key = if per_message {
            "-id"
        } else {
            "coalesce(thread_id, -id)"
        };
        let mkey = if per_message {
            "-m.id"
        } else {
            "coalesce(m.thread_id, -m.id)"
        };
        let sql = format!(
            "SELECT coalesce(m.thread_id, -m.id), m.id, coalesce(m.from_display,''), coalesce(m.from_addr,''),
                    coalesce(m.subject,''), coalesce(m.snippet,''), m.date_ms, t.n,
                    coalesce(t.participants,''), t.unread, t.starred, t.attach,
                    {tags_json},
                    (SELECT a.filename FROM attachments a
                     JOIN messages mm ON mm.id = a.message_id
                     WHERE coalesce(mm.thread_id, -mm.id) = t.thread_id
                       AND a.filename IS NOT NULL AND a.filename <> ''
                     ORDER BY a.id LIMIT 1) AS attach_name
             FROM messages m
             JOIN (
               SELECT {key} AS thread_id, max(date_ms) AS md, count(*) AS n,
                      group_concat(DISTINCT coalesce(nullif(from_display,''), from_addr))
                        AS participants,
                      max(CASE WHEN flags & 1 = 0 THEN 1 ELSE 0 END) AS unread,
                      max(CASE WHEN flags & 4 != 0 THEN 1 ELSE 0 END) AS starred,
                      max(has_attachments) AS attach
               FROM messages WHERE deleted_at_ms IS NULL AND account_id = {account} AND {inner}
                 AND {key} IN ({holes})
               GROUP BY {key}
             ) t ON {mkey} = t.thread_id AND m.date_ms = t.md
             WHERE m.deleted_at_ms IS NULL AND m.account_id = {account} AND {outer}
               AND {mkey} IN ({holes})
             GROUP BY {mkey}
             ORDER BY m.date_ms DESC LIMIT ?1 OFFSET ?2",
            inner = inner,
            tags_json = TAGS_JSON,
            outer = outer,
            account = account,
            key = key,
            mkey = mkey,
            // Written into the SQL rather than bound: these are row ids this
            // query just read out of the database, and mixing anonymous
            // placeholders with the predicate's numbered ?3 is how a key
            // silently gets bound as a folder role.
            holes = keys
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
                .join(","),
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
        let mut out = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        // Back into the order the page was chosen in. The query above ends in
        // ORDER BY date DESC, which is not the list's order and cannot be:
        // it groups, and saying "the order these ids arrived in" in SQL means
        // a CASE with one branch per row.
        // The query above groups
        // and therefore cannot preserve it, and for a long time it did not
        // have to: every list was newest-first and the two orders agreed by
        // accident. The moment a list could be sorted any other way, that
        // accident became a list that ignored the sort it was given.
        let rank: std::collections::HashMap<i64, usize> =
            keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        out.sort_by_key(|r| rank.get(&r.thread_id).copied().unwrap_or(usize::MAX));
        Ok(out)
    }

    /// The numbers beside the rail's mailboxes.
    ///
    /// Counted per *conversation*, not per message: the list shows
    /// conversations, so a five-message thread holding one unread message is
    /// one unread row. A badge saying five would not match anything the user
    /// can point at.
    ///
    /// One rule: each mailbox reports what is *waiting* in it.
    ///
    /// Unread is only what waiting means in the Inbox, and in folders and the
    /// bins, where mail arrives on its own and the question is whether you have
    /// looked at it. It means something else on a list you made yourself.
    /// Starring, snoozing and writing a draft are all acts of putting something
    /// aside on purpose, so everything on those lists is waiting whether or not
    /// it has been read — reading a starred message does not take it off the
    /// list. Nothing ever waits in Sent, so Sent gets no number; a count that
    /// never moves is furniture.
    ///
    /// Stated as one rule because the old spelling of it — unread everywhere,
    /// with a growing list of exceptions — had four exceptions and a setting
    /// labelled "Unread" that contradicted them. A real account showed no
    /// badge at all beside eighteen starred conversations.
    /// The caller may override any of it per mailbox — that is the sidebar
    /// section's whole job — and anything it does not name falls to the rule
    /// above. `folders` covers every folder somebody made, which have no
    /// individual rows of their own in that section.
    pub fn view_counts(
        &self,
        modes: &std::collections::HashMap<String, CountMode>,
    ) -> Result<Vec<(String, i64)>> {
        let mode_for = |key: &str| {
            modes
                .get(key)
                .copied()
                .unwrap_or_else(|| Self::default_count_mode(key))
        };

        let mut out = Vec::new();
        for key in MAILBOX_KEYS {
            let mode = mode_for(key);
            if mode == CountMode::Off {
                continue;
            }
            let n = self.count_view(&ListView::parse(key), mode == CountMode::Total)?;
            if n > 0 {
                out.push((key.to_string(), n));
            }
        }
        // Tags are left out: their rail rows already carry a count, and it is
        // a total rather than an unread one. "How much is tagged Urgent" is the
        // question a tag answers, and swapping it for an unread count would
        // lose that to make two different things look alike.
        // One more row, under its own key: how many outbox messages are
        // waiting on a person. Reported alongside the counts rather than by a
        // separate call so the rail learns of it on the same cadence it learns
        // everything else — a message that needs a decision must not go
        // unnoticed, and a count fetched only while the Outbox is open would
        // be exactly the way it did.
        let needs: i64 = self.conn.query_row(
            "SELECT count(*) FROM messages
              WHERE send_after_ms IS NOT NULL AND send_state = 'NeedsAttention'
                AND account_id = ?1",
            [self.active_account()?.unwrap_or(-1)],
            |r| r.get(0),
        )?;
        // Folders the user made get the same unread badge the mailboxes
        // wear. One grouped query for all of them: the first version ran a
        // full thread-grouping count per folder, and forty folders times a
        // six-thousand-message account, refreshed on every sync tick, was a
        // measurable share of what made the app feel stuck mid-sync.
        // Folders answer under one key of their own. Mail lands in them by
        // itself, so unread is what waiting means there.
        let folders_mode = mode_for("folders");
        if let Some(account) = self.active_account()?
            && folders_mode != CountMode::Off
        {
            let unread_clause = match folders_mode {
                CountMode::Total | CountMode::Off => String::new(),
                CountMode::Unread => format!(" AND m.flags & {} = 0", flags::SEEN),
            };
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT p.folder_id, count(DISTINCT coalesce(m.thread_id, -m.id))
                 FROM placements p
                 JOIN folders f ON f.id = p.folder_id
                 JOIN messages m ON m.id = p.message_id
                 WHERE f.account_id = ?1 AND coalesce(f.role,'') = ''
                   AND m.deleted_at_ms IS NULL{unread_clause}
                 GROUP BY p.folder_id",
            ))?;
            let rows = stmt.query_map(params![account], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (fid, n) = row?;
                if n > 0 {
                    out.push((format!("folder:{fid}"), n));
                }
            }
        }
        // Only when there is something to say. This one is not a count and no
        // count preference silences it: a message whose send could not be
        // proved either way is a problem, not a badge, and somebody who asked
        // for no numbers did not ask to be kept in the dark about it. Absent
        // rather than zero because the rail replaces this map wholesale, so a
        // missing key clears the amber exactly as a zero would.
        if needs > 0 {
            out.push(("outbox:attention".to_string(), needs));
        }
        Ok(out)
    }

    /// The whole of a view, counted — what the status line reports, where
    /// the loaded list is a page and its length is not a fact about the
    /// mailbox.
    pub fn conversations_in(&self, view: &ListView) -> Result<i64> {
        self.count_view(view, true)
    }

    /// Conversations in a view: all of them, or only those holding something
    /// unread.
    fn count_view(&self, view: &ListView, total: bool) -> Result<i64> {
        if let Some(n) = self.count_view_from_placements(view, total)? {
            return Ok(n);
        }
        let having = if total {
            String::new()
        } else {
            format!(
                "HAVING max(CASE WHEN flags & {seen} = 0 THEN 1 ELSE 0 END) = 1",
                seen = flags::SEEN
            )
        };
        let ckey = if matches!(view, ListView::Folder(r) if r == "drafts") {
            "-id"
        } else {
            "coalesce(thread_id, -id)"
        };
        let sql = format!(
            "SELECT count(*) FROM (
               SELECT {ckey} AS tid
               FROM messages
               WHERE deleted_at_ms IS NULL AND account_id = {account} AND {pred}
               GROUP BY {ckey}
               {having}
             )",
            account = self.active_account()?.unwrap_or(-1),
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

    /// Counts that can start from a folder's placements rather than from every
    /// message with a correlated EXISTS. Inbox, the role folders, and user
    /// folders are membership in a placement; scanning the whole mailbox to
    /// ask each row whether it sits in INBOX is the slow form of the same join.
    ///
    /// The ids are materialized first. GROUP BY on `coalesce(thread_id, -id)`
    /// otherwise makes SQLite drive from `idx_messages_account_thread` and
    /// probe placements per row — at a couple of hundred thousand messages
    /// that is a 300ms count of thirteen drafts.
    fn count_view_from_placements(&self, view: &ListView, total: bool) -> Result<Option<i64>> {
        let account = self.active_account()?.unwrap_or(-1);
        let n = match view {
            ListView::Inbox => self.count_from_message_ids(
                account,
                &format!(
                    "SELECT p.message_id
                       FROM folders f
                       JOIN placements p ON p.folder_id = f.id
                      WHERE f.account_id = {account} AND f.role = 'inbox'"
                ),
                &format!(
                    "AND (m.snoozed_until_ms IS NULL OR m.snoozed_until_ms <= (strftime('%s','now') * 1000))
                     AND {not_binned}",
                    not_binned = not_binned("m"),
                ),
                "coalesce(m.thread_id, -m.id)",
                total,
                None,
            )?,
            ListView::UserFolder(id) => self.count_from_message_ids(
                account,
                &format!("SELECT message_id FROM placements WHERE folder_id = {id}"),
                "",
                "coalesce(m.thread_id, -m.id)",
                total,
                None,
            )?,
            ListView::Folder(role) if role == "drafts" => self.count_from_message_ids(
                account,
                &format!(
                    "SELECT p.message_id
                       FROM folders f
                       JOIN placements p ON p.folder_id = f.id
                      WHERE f.account_id = {account} AND f.role = ?3"
                ),
                "AND m.send_after_ms IS NULL",
                "-m.id",
                total,
                Some(role.as_str()),
            )?,
            ListView::Folder(role) if role == "archive" => self.count_from_message_ids(
                account,
                &format!(
                    "SELECT p.message_id
                       FROM folders f
                       JOIN placements p ON p.folder_id = f.id
                      WHERE f.account_id = {account}
                        AND (f.role = 'archive'
                             OR EXISTS (SELECT 1 FROM folders af
                                        WHERE af.role = 'archive'
                                          AND af.account_id = f.account_id
                                          AND (f.path LIKE af.path || '/%'
                                               OR f.path LIKE af.path || '.%')))"
                ),
                "AND NOT EXISTS (SELECT 1 FROM placements p2
                                 JOIN folders f2 ON f2.id = p2.folder_id
                                 WHERE p2.message_id = m.id AND f2.role = 'inbox')",
                "coalesce(m.thread_id, -m.id)",
                total,
                None,
            )?,
            ListView::Folder(role) => self.count_from_message_ids(
                account,
                &format!(
                    "SELECT p.message_id
                       FROM folders f
                       JOIN placements p ON p.folder_id = f.id
                      WHERE f.account_id = {account} AND f.role = ?3"
                ),
                "",
                "coalesce(m.thread_id, -m.id)",
                total,
                Some(role.as_str()),
            )?,
            ListView::Starred => self.count_on_partial_index(
                account,
                "idx_messages_flagged",
                &format!(
                    "flags & {flagged} != 0 AND {binned}",
                    flagged = flags::FLAGGED,
                    binned = not_binned("messages"),
                ),
                total,
            )?,
            ListView::Snoozed => self.count_on_partial_index(
                account,
                "idx_messages_snoozed",
                "snoozed_until_ms IS NOT NULL
                 AND snoozed_until_ms > (strftime('%s','now') * 1000)",
                total,
            )?,
            ListView::Outbox => self.count_on_partial_index(
                account,
                "idx_messages_send_after",
                "send_after_ms IS NOT NULL",
                total,
            )?,
            _ => return Ok(None),
        };
        Ok(Some(n))
    }

    fn count_from_message_ids(
        &self,
        account: i64,
        ids_sql: &str,
        extra_where: &str,
        thread_key: &str,
        total: bool,
        bound: Option<&str>,
    ) -> Result<i64> {
        let having = if total {
            String::new()
        } else {
            format!(
                "HAVING max(CASE WHEN m.flags & {seen} = 0 THEN 1 ELSE 0 END) = 1",
                seen = flags::SEEN
            )
        };
        let sql = format!(
            "WITH ids AS MATERIALIZED ({ids_sql})
             SELECT count(*) FROM (
               SELECT {thread_key} AS tid
               FROM ids
               JOIN messages m ON m.id = ids.message_id
               WHERE m.deleted_at_ms IS NULL AND m.account_id = {account}
                 {extra_where}
               GROUP BY tid
               {having}
             )"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        Ok(match bound {
            Some(role) => stmt.query_row(
                rusqlite::params![rusqlite::types::Null, rusqlite::types::Null, role],
                |r| r.get(0),
            )?,
            None => stmt.query_row([], |r| r.get(0))?,
        })
    }

    /// Walk a partial index instead of the account's live messages.
    ///
    /// `INDEXED BY` is load-bearing: at mailbox scale the planner prefers
    /// `idx_messages_account_thread` for any `account_id = ?` filter, and a
    /// count of zero snoozed conversations still reads every row.
    fn count_on_partial_index(
        &self,
        account: i64,
        index: &str,
        pred: &str,
        total: bool,
    ) -> Result<i64> {
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
               FROM messages INDEXED BY {index}
               WHERE deleted_at_ms IS NULL AND account_id = {account}
                 AND {pred}
               GROUP BY tid
               {having}
             )"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        Ok(stmt.query_row([], |r| r.get(0))?)
    }

    /// The thread-row aggregate, restricted to a set of conversations.
    pub(super) fn threads_by_id(&self, thread_ids: &[i64]) -> Result<Vec<ThreadListing>> {
        if thread_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Written into the SQL rather than bound: these are row ids this
        // query just read out of the database, and the inner GROUP BY and
        // the outer filter both name the same set — binding the list once
        // left half the placeholders hungry.
        let holes = thread_ids
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let tags_json = TAGS_JSON;
        let sql = format!(
            "SELECT coalesce(m.thread_id, -m.id), m.id, coalesce(m.from_display,''), coalesce(m.from_addr,''),
                    coalesce(m.subject,''), coalesce(m.snippet,''), m.date_ms, t.n,
                    coalesce(t.participants,''), t.unread, t.starred, t.attach,
                    {tags_json},
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
                 AND coalesce(thread_id, -id) IN ({holes})
               GROUP BY coalesce(thread_id, -id)
             ) t ON coalesce(m.thread_id, -m.id) = t.thread_id AND m.date_ms = t.md
             WHERE m.deleted_at_ms IS NULL AND coalesce(m.thread_id, -m.id) IN ({holes})
             GROUP BY coalesce(m.thread_id, -m.id)"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
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

    /// One conversation, every message. Invitations and tests still need the
    /// whole thread; the reading pane must not call this.
    pub fn thread_detail(&self, thread_id: i64) -> Result<Vec<ThreadMessage>> {
        self.thread_detail_page(thread_id, None, None)
    }

    /// Sender, snippet and date for every surviving message in a conversation.
    ///
    /// One query, no recipients or attachments. The reading pane virtualizes
    /// these rows and hydrates a body only when a card is opened.
    pub fn thread_index(&self, thread_id: i64) -> Result<Vec<ThreadIndexRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(snippet,''), date_ms, flags
             FROM messages
             WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL
             ORDER BY date_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![thread_id], |r| {
            let flags: i64 = r.get(5)?;
            Ok(ThreadIndexRow {
                id: r.get(0)?,
                from_display: r.get(1)?,
                from_addr: r.get(2)?,
                snippet: r.get(3)?,
                date_ms: r.get(4)?,
                unread: flags & flags::SEEN == 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// One message, fully hydrated — the reading pane asks for this when a
    /// card is opened. Missing or deleted is `None`, not an error.
    pub fn thread_message(&self, message_id: i64) -> Result<Option<ThreadMessage>> {
        const COLS: &str = "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), coalesce(snippet,''), date_ms, flags,
                    EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = messages.id
                              AND (a.mime LIKE '%calendar%' OR a.mime = 'application/ics'
                                   OR lower(coalesce(a.filename,'')) LIKE '%.ics')),
                    invite_response, message_id_hdr
             FROM messages
             WHERE id = ?1 AND deleted_at_ms IS NULL";
        let mut stmt = self.conn.prepare_cached(COLS)?;
        let row = stmt
            .query_row(params![message_id], thread_detail_row)
            .optional()?;
        match row {
            Some(r) => Ok(self.hydrate_thread_messages(vec![r])?.into_iter().next()),
            None => Ok(None),
        }
    }

    /// One conversation, a page at a time.
    ///
    /// Newest-first when `limit` is set. `before` is the oldest row already
    /// shown (`date_ms`, `id`), exclusive, walking older. Rows come back
    /// oldest-first so a caller can prepend without re-sorting. The reading
    /// pane uses [`Self::thread_index`]; this stays for invitations and tests.
    pub fn thread_detail_page(
        &self,
        thread_id: i64,
        limit: Option<u32>,
        before: Option<(i64, i64)>,
    ) -> Result<Vec<ThreadMessage>> {
        const COLS: &str = "SELECT id, coalesce(from_display,''), coalesce(from_addr,''),
                    coalesce(subject,''), coalesce(snippet,''), date_ms, flags,
                    EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = messages.id
                              AND (a.mime LIKE '%calendar%' OR a.mime = 'application/ics'
                                   OR lower(coalesce(a.filename,'')) LIKE '%.ics')),
                    invite_response, message_id_hdr
             FROM messages
             WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL";

        let rows: Vec<ThreadDetailRow> = match (limit, before) {
            (None, None) => {
                let mut stmt = self
                    .conn
                    .prepare_cached(&format!("{COLS} ORDER BY date_ms ASC, id ASC"))?;
                stmt.query_map(params![thread_id], thread_detail_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (Some(n), None) => {
                let mut stmt = self
                    .conn
                    .prepare_cached(&format!("{COLS} ORDER BY date_ms DESC, id DESC LIMIT ?2"))?;
                let mut page = stmt
                    .query_map(params![thread_id, n], thread_detail_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                page.reverse();
                page
            }
            (n, Some((date_ms, id))) => {
                let take = n.unwrap_or(u32::MAX);
                let mut stmt = self.conn.prepare_cached(&format!(
                    "{COLS} AND (date_ms < ?2 OR (date_ms = ?2 AND id < ?3))
                     ORDER BY date_ms DESC, id DESC LIMIT ?4"
                ))?;
                let mut page = stmt
                    .query_map(params![thread_id, date_ms, id, take], thread_detail_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                page.reverse();
                page
            }
        };

        self.hydrate_thread_messages(rows)
    }

    fn addresses_for_messages(&self, ids: &[i64]) -> Result<HashMap<i64, MessageAddrs>> {
        let mut out: HashMap<i64, MessageAddrs> = HashMap::new();
        for chunk in ids.chunks(HYDRATE_IN_CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT message_id, role, coalesce(nullif(display,''), addr_norm), addr_norm
                 FROM message_addresses
                 WHERE message_id IN ({})
                 ORDER BY message_id, rowid",
                sql_in_marks(chunk.len())
            );
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (message_id, role, display, addr) = row?;
                let entry = out.entry(message_id).or_default();
                if role == "to" {
                    entry.to.push((display, addr));
                } else if role == "cc" {
                    entry.cc.push((display, addr));
                }
            }
        }
        Ok(out)
    }

    fn attachments_for_messages(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<Attachment>>> {
        let mut out: HashMap<i64, Vec<Attachment>> = HashMap::new();
        for chunk in ids.chunks(HYDRATE_IN_CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT message_id, coalesce(filename,''), coalesce(size, 0), part_id,
                        coalesce(mime,'')
                 FROM attachments
                 WHERE message_id IN ({})
                   AND filename IS NOT NULL AND filename <> ''
                 ORDER BY message_id, id",
                sql_in_marks(chunk.len())
            );
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    Attachment {
                        filename: r.get(1)?,
                        size: r.get(2)?,
                        part: r.get(3)?,
                        mime: r.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (message_id, att) = row?;
                out.entry(message_id).or_default().push(att);
            }
        }
        Ok(out)
    }

    /// The ids each message referenced, as ingest recorded them.
    fn references_for_messages(&self, ids: &[i64]) -> Result<HashMap<i64, Vec<String>>> {
        let mut out: HashMap<i64, Vec<String>> = HashMap::new();
        for chunk in ids.chunks(HYDRATE_IN_CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT message_id, ref_msgid FROM message_refs
                 WHERE message_id IN ({})
                 ORDER BY message_id, ref_msgid",
                sql_in_marks(chunk.len())
            );
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (message_id, msgid) = row?;
                out.entry(message_id).or_default().push(msgid);
            }
        }
        Ok(out)
    }

    fn hydrate_thread_messages(&self, rows: Vec<ThreadDetailRow>) -> Result<Vec<ThreadMessage>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let addrs = self.addresses_for_messages(&ids)?;
        let files = self.attachments_for_messages(&ids)?;
        let mut refs = self.references_for_messages(&ids)?;
        let mut out = Vec::with_capacity(rows.len());
        for (
            id,
            from_display,
            from_addr,
            subject,
            snippet,
            date_ms,
            flags,
            has_calendar,
            invite_response,
            msgid_key,
        ) in rows
        {
            let buckets = addrs.get(&id).cloned().unwrap_or_default();
            let to: Vec<String> = buckets.to.iter().map(|(d, _)| d.clone()).collect();
            let cc: Vec<String> = buckets.cc.iter().map(|(d, _)| d.clone()).collect();
            let recipients: Vec<String> = to.iter().chain(cc.iter()).cloned().collect();
            let recipient_addrs: Vec<String> = buckets
                .to
                .into_iter()
                .chain(buckets.cc)
                .map(|(_, a)| a)
                .collect();
            let attachments = files.get(&id).cloned().unwrap_or_default();

            out.push(ThreadMessage {
                id,
                from_display,
                from_addr,
                subject,
                snippet,
                date_ms,
                unread: flags & flags::SEEN == 0,
                has_calendar,
                invite_response,
                to,
                cc,
                recipients,
                recipient_addrs,
                attachments,
                msgid: wire_msgid(msgid_key),
                references: refs.remove(&id).unwrap_or_default(),
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
}
