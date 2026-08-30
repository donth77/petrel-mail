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
            ..
        } = q;
        let (limit, offset, per_message, sort) = (*limit, *offset, *per_message, *sort);
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
            SortKey::Date => {
                let dir = if sort.ascending { "ASC" } else { "DESC" };
                format!(
                    "SELECT {key} FROM messages
                     WHERE deleted_at_ms IS NULL AND account_id = {account} AND {inner}
                     ORDER BY date_ms {dir}"
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
                    "SELECT n.k FROM (
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
