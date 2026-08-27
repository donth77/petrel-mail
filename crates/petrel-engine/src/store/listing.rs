//! The conversation list: rows, one conversation by id, its messages, and
//! the counts the rail wears.
//!
//! Moved verbatim from mod.rs (Phase 1.5). The listing SQL and the counts
//! share the ListView predicates, which stay in mod.rs with the enum.
use super::*;

impl Store {
    pub fn list_threads(
        &self,
        view: &ListView,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ThreadListing>> {
        // Scoped to the account on screen. The query was written when one
        // account was all there was; with two, every view showed both
        // mailboxes merged — which is exactly the send-from-the-wrong-address
        // mistake that "one active at a time" exists to prevent. A missing
        // account (an empty store) scopes to nothing, which lists nothing.
        let account = self.active_account()?.unwrap_or(-1);
        self.list_threads_for(account, view, offset, limit)
    }

    /// `list_threads` for a named account rather than the active one — for
    /// callers like export that act on a mailbox the window is not showing.
    pub fn list_threads_for(
        &self,
        account: i64,
        view: &ListView,
        offset: u32,
        limit: u32,
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
}

impl Store {
    /// The conversation-list query, shared by the views and by a lookup of one.
    ///
    /// Two steps, and the order is the whole performance story. Aggregating
    /// first and paging afterwards means grouping every message in the
    /// mailbox — participants, counts, flags — to show fifty rows: at a
    /// hundred thousand messages that measured 562ms to open a list, against
    /// a 150ms budget, and no index helps because the work is real. So the
    /// page is chosen first, by walking newest-first down an index and
    /// collecting distinct conversations until there are enough, and only
    /// those conversations are aggregated.
    fn listing_rows(&self, account: i64, q: ListingQuery<'_>) -> Result<Vec<ThreadListing>> {
        let ListingQuery {
            inner,
            outer,
            limit,
            offset,
            bound,
            per_message,
        } = q;
        let keys = self.page_keys(account, inner, per_message, limit, offset, bound.clone())?;
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        self.rows_for_keys(account, inner, outer, per_message, &keys, bound)
    }

    /// The conversations one page of the list shows, newest first.
    ///
    /// Walks messages down `(account_id, date_ms DESC)` and keeps the first
    /// sighting of each conversation, stopping the moment the page is full —
    /// SQLite streams the rows, so a mailbox of any size costs about a page's
    /// worth of them. A conversation's *position* is its newest message,
    /// which is exactly what walking newest-first gives.
    fn page_keys(
        &self,
        account: i64,
        inner: &str,
        per_message: bool,
        limit: u32,
        offset: u32,
        bound: Option<String>,
    ) -> Result<Vec<i64>> {
        let key = if per_message {
            "-id"
        } else {
            "coalesce(thread_id, -id)"
        };
        let sql = format!(
            "SELECT {key} FROM messages
             WHERE deleted_at_ms IS NULL AND account_id = {account} AND {inner}
             ORDER BY date_ms DESC"
        );
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
        let want = (offset as usize).saturating_add(limit as usize);
        let mut seen: std::collections::HashSet<i64> = Default::default();
        let mut ordered: Vec<i64> = Vec::with_capacity(want.min(1024));
        while let Some(row) = rows.next()? {
            let k: i64 = row.get(0)?;
            if seen.insert(k) {
                ordered.push(k);
                if ordered.len() >= want {
                    break;
                }
            }
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
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
        if let Some(account) = self.active_account()? {
            // The same mode the mailboxes answer in: unread by default, or
            // everything when the badge setting says so.
            let unread_clause = match mode {
                // Off never reaches here (the fn returns empty above), but
                // exhaustiveness is cheaper than the assumption.
                CountMode::Off => return Ok(out),
                CountMode::Total => String::new(),
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
        out.push(("outbox:attention".to_string(), needs));
        Ok(out)
    }

    /// The whole of a view, counted — what the status line reports, where
    /// the loaded list is a 500-row window and its length is not a fact
    /// about the mailbox.
    pub fn conversations_in(&self, view: &ListView) -> Result<i64> {
        self.count_view(view, true)
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

    /// The thread-row aggregate, restricted to a set of conversations.
    pub(super) fn threads_by_id(&self, thread_ids: &[i64]) -> Result<Vec<ThreadListing>> {
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
                    coalesce(subject,''), coalesce(snippet,''), date_ms, flags,
                    EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = messages.id
                              AND (a.mime LIKE '%calendar%' OR a.mime = 'application/ics'
                                   OR lower(coalesce(a.filename,'')) LIKE '%.ics')),
                    invite_response
             FROM messages
             WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL
             ORDER BY date_ms ASC",
        )?;
        type Row = (
            i64,
            String,
            String,
            String,
            String,
            i64,
            i64,
            bool,
            Option<String>,
        );
        let rows: Vec<Row> = stmt
            .query_map(params![thread_id], |r| {
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
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

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
        ) in rows
        {
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
                "SELECT coalesce(filename,''), coalesce(size, 0), part_id, coalesce(mime,'')
                 FROM attachments
                 WHERE message_id = ?1 AND filename IS NOT NULL AND filename <> ''
                 ORDER BY id",
            )?;
            let attachments: Vec<Attachment> = att
                .query_map(params![id], |r| {
                    Ok(Attachment {
                        filename: r.get(0)?,
                        size: r.get(1)?,
                        part: r.get(2)?,
                        mime: r.get(3)?,
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
                has_calendar,
                invite_response,
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
}
