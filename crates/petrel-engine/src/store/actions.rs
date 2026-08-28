//! Triage actions and tags: the queue of intent, its undo, and the labels
//! mail wears — including the Gmail sweep that keeps both worlds agreeing.
//!
//! Moved verbatim from mod.rs (Phase 1.5).
use super::*;

impl Store {
    /// The queued actions, oldest first, with the UID and folder each one needs
    /// to reach the server.
    ///
    /// Oldest first matters: two actions on the same message must arrive in the
    /// order the user performed them, or the later one loses.
    pub fn pending_actions(&self, account_id: i64) -> Result<Vec<PendingAction>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.kind, a.payload_json, am.message_id, p.uid, f.path,
                    m.message_id_hdr
             FROM actions a
             JOIN action_messages am ON am.action_id = a.id
             JOIN messages m ON m.id = am.message_id
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
                msgid: r.get::<_, Option<String>>(6)?,
                candidate_paths: Vec::new(),
            })
        })?;
        let mut out: Vec<PendingAction> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        // A move deleted the placement its own delivery needed; the address
        // captured at queue time is the fallback that makes such an action
        // deliverable at all.
        for row in &mut out {
            if row.uid.is_some() {
                continue;
            }
            let Some(payload) =
                serde_json::from_str::<crate::actions::ActionPayload>(&row.payload_json).ok()
            else {
                continue;
            };
            let prior = payload
                .prior
                .iter()
                .find(|p| p.message_id == row.message_id);
            if let Some(prior) = prior
                && let (Some(path), Some(uid)) = (&prior.source_path, prior.source_uid)
            {
                // Only when the source placement is *gone* — a move deleted
                // it, taking the address with it. A placement that still
                // exists with its UID nulled is UIDVALIDITY quarantine, and
                // quarantine means exactly "this number is a lie now": the
                // captured address must stay unusable until recovery re-maps.
                let quarantined: bool = self
                    .conn
                    .query_row(
                        "SELECT 1 FROM placements p
                         JOIN folders f ON f.id = p.folder_id
                         WHERE p.message_id = ?1 AND f.path = ?2 AND f.account_id = ?3",
                        params![row.message_id, path, account_id],
                        |_| Ok(true),
                    )
                    .optional()?
                    .unwrap_or(false);
                if !quarantined {
                    row.uid = Some(uid);
                    row.folder_path = path.clone();
                }
            }
            // Still no address. Gather the folders a Message-ID search could
            // ask — the queue-time folders first, because for a move that is
            // where the server copy still sits, then wherever a placement
            // still holds the message, which for a quarantined number is the
            // folder whose renumbering took the address away.
            if row.uid.is_none() {
                let mut paths: Vec<String> = Vec::new();
                if let Some(prior) = prior {
                    for fid in &prior.folder_ids {
                        if let Some(p) = self.folder_path(*fid)?
                            && !paths.contains(&p)
                        {
                            paths.push(p);
                        }
                    }
                }
                if !row.folder_path.is_empty() && !paths.contains(&row.folder_path) {
                    paths.push(row.folder_path.clone());
                }
                row.candidate_paths = paths;
            }
        }
        Ok(out)
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
            // The server address rides with the action from birth. A move is
            // about to delete the placement row that holds it, and delivery
            // read the row at drain time — so a delivered-after-move queue
            // was structurally impossible.
            let source: Option<(String, i64)> = self
                .conn
                .query_row(
                    "SELECT f.path, p.uid FROM placements p
                     JOIN folders f ON f.id = p.folder_id
                     WHERE p.message_id = ?1 AND p.uid IS NOT NULL
                     ORDER BY (f.role = 'inbox') DESC LIMIT 1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
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
                source_path: source.as_ref().map(|(p, _)| p.clone()),
                source_uid: source.as_ref().map(|(_, u)| *u as u32),
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

    /// Tombstones one message the way delete-forever does: out of search at
    /// once, bytes reaped by the grace-period sweep rather than here.
    pub fn tombstone_message(&self, message_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET deleted_at_ms = (strftime('%s','now') * 1000)
             WHERE id = ?1 AND deleted_at_ms IS NULL",
            params![message_id],
        )?;
        self.conn.execute(
            "DELETE FROM fts_content WHERE message_id = ?1",
            params![message_id],
        )?;
        self.conn.execute(
            "DELETE FROM placements WHERE message_id = ?1",
            params![message_id],
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
             GROUP BY t.id
             ORDER BY (t.sort_order IS NULL), t.sort_order, t.name COLLATE NOCASE",
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
    /// Records Gmail's own conversation id for the messages a sweep named.
    /// Pairs are `(uid, thrid)` within the given folder — All Mail, in
    /// practice, since every Gmail message lives there.
    pub fn apply_gm_thrids(&self, folder_id: i64, pairs: &[(u32, u64)]) -> Result<usize> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE messages SET gm_thrid = ?2
             WHERE id = (SELECT message_id FROM placements
                          WHERE folder_id = ?1 AND uid = ?3)
               AND (gm_thrid IS NULL OR gm_thrid != ?2)",
        )?;
        let mut n = 0usize;
        for (uid, thrid) in pairs {
            n += stmt.execute(params![folder_id, *thrid as i64, *uid as i64])?;
        }
        Ok(n)
    }

    /// Makes Gmail's word on conversations the store's word.
    ///
    /// Every message with a known X-GM-THRID moves to the thread canonical
    /// for that id — the lowest message row id in the group, the same
    /// convention local threading uses. This both merges what References
    /// could not connect and splits what a shared subject wrongly glued.
    /// Messages with no thrid yet keep their local threading until a sweep
    /// names them.
    pub fn regroup_gmail_threads(&self, account_id: i64) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE messages SET thread_id = (
                 SELECT min(m2.id) FROM messages m2
                  WHERE m2.account_id = messages.account_id
                    AND m2.gm_thrid = messages.gm_thrid)
             WHERE account_id = ?1 AND gm_thrid IS NOT NULL
               AND coalesce(thread_id, -id) != (
                 SELECT min(m2.id) FROM messages m2
                  WHERE m2.account_id = messages.account_id
                    AND m2.gm_thrid = messages.gm_thrid)",
            params![account_id],
        )?;
        Ok(n)
    }

    /// Makes the server's keywords this account's tags, for one folder's
    /// worth of UIDs.
    ///
    /// The write path munges a tag's name to an atom (`keywords::tag_keyword`),
    /// so the way back matches on that munge rather than on the raw atom: a
    /// tag called "Waiting on" travels as `Waiting_on` and must come home as
    /// "Waiting on", not as a second tag spelled with an underscore. A keyword
    /// no existing tag munges to is a tag made elsewhere, and becomes one here
    /// under the atom's own name.
    ///
    /// Reconciles rather than adds: a keyword the message no longer wears
    /// loses its tag, which is what makes untagging in another client mean
    /// something here. Only tags that are keyword-shaped are touched — a tag
    /// that has never been to the server keeps its own life.
    pub fn apply_keywords(
        &self,
        account_id: i64,
        folder_id: i64,
        keyworded: &[(u32, Vec<String>)],
    ) -> Result<usize> {
        let tags: Vec<(String, i64)> = self
            .tags_for_account(account_id)?
            .into_iter()
            .map(|t| (t.name, t.id))
            .collect();
        let mut changed = 0usize;
        for (uid, keywords) in keyworded {
            let message: Option<i64> = self
                .conn
                .query_row(
                    "SELECT message_id FROM placements WHERE folder_id = ?1 AND uid = ?2",
                    params![folder_id, *uid as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(message_id) = message else { continue };

            // What this message should wear, by tag id.
            let mut want: std::collections::BTreeSet<i64> = Default::default();
            for kw in keywords {
                let existing = tags
                    .iter()
                    .find(|(name, _)| crate::keywords::tag_keyword(name) == *kw)
                    .map(|(_, id)| *id);
                let id = match existing {
                    Some(id) => id,
                    None => self.ensure_tag(account_id, kw, None)?,
                };
                want.insert(id);
            }

            // What it wears now, keeping only the keyword-shaped ones in view:
            // a tag that never travelled is not the server's to remove.
            let mut have = self.conn.prepare_cached(
                "SELECT t.id, t.name FROM message_tags mt
                 JOIN tags t ON t.id = mt.tag_id
                 WHERE mt.message_id = ?1 AND t.account_id = ?2",
            )?;
            let current: Vec<(i64, String)> = have
                .query_map(params![message_id, account_id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for id in &want {
                if !current.iter().any(|(cid, _)| cid == id) {
                    self.tag_message(message_id, *id)?;
                    changed += 1;
                }
            }
            for (id, name) in &current {
                if !want.contains(id)
                    && keywords
                        .iter()
                        .all(|k| *k != crate::keywords::tag_keyword(name))
                {
                    self.untag_message(message_id, *id)?;
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

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
        let tags: Vec<(String, i64)> = self
            .tags_for_account(account_id)?
            .into_iter()
            .map(|t| (t.name, t.id))
            .collect();
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

            // Labels that are Petrel tags sync their membership both ways.
            // No label changes category here: one made as a tag stays a tag,
            // everything else stays a folder — this only makes "tagged in
            // Gmail's web UI" and "tagged here" the same fact. System labels
            // arrive backslash-prefixed and are never tag material.
            for (tag_name, tag_id) in &tags {
                let carried = labels.iter().any(|l| {
                    let name = l.trim_matches('"').trim_start_matches('\\');
                    !l.trim_matches('"').starts_with('\\') && name == tag_name
                });
                if carried {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO message_tags(message_id, tag_id) VALUES (?1, ?2)",
                        params![id, tag_id],
                    )?;
                } else {
                    self.conn.execute(
                        "DELETE FROM message_tags WHERE message_id = ?1 AND tag_id = ?2",
                        params![id, tag_id],
                    )?;
                }
            }
            changed += 1;
        }
        Ok(changed)
    }
}
