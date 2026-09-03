//! Triage actions and tags: the queue of intent, its undo, and the labels
//! mail wears — including the Gmail sweep that keeps both worlds agreeing.
//!
//! Moved verbatim from mod.rs (Phase 1.5).
use super::*;
use std::collections::HashMap;
use std::sync::Arc;

impl Store {
    /// The queued actions, oldest first, with the UID and folder each one needs
    /// to reach the server.
    ///
    /// Oldest first matters: two actions on the same message must arrive in the
    /// order the user performed them, or the later one loses.
    pub fn pending_actions(&self, account_id: i64) -> Result<Vec<PendingAction>> {
        // Payload is read once per action. The JOIN used to select it on every
        // action_messages row, so a mark_read of a 22k-message thread cloned a
        // 2.8MB undo snapshot twenty thousand times and the drain held ~60GB.
        let mut payloads: HashMap<i64, Arc<str>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, payload_json FROM actions
                 WHERE account_id = ?1 AND state = 'queued'",
            )?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, json) = row?;
                payloads.insert(id, Arc::from(json));
            }
        }
        let empty: Arc<str> = Arc::from("{}");
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.kind, am.message_id, p.uid, f.path,
                    m.message_id_hdr
             FROM actions a
             JOIN action_messages am ON am.action_id = a.id
             JOIN messages m ON m.id = am.message_id
             LEFT JOIN placements p ON p.message_id = am.message_id
             LEFT JOIN folders f ON f.id = p.folder_id
             WHERE a.account_id = ?1 AND a.state = 'queued'
               AND am.delivered_ms IS NULL AND am.dropped_ms IS NULL
             ORDER BY a.id, am.message_id",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            let action_id: i64 = r.get(0)?;
            Ok(PendingAction {
                action_id,
                kind_json: r.get(1)?,
                payload_json: empty.clone(),
                message_id: r.get(2)?,
                uid: r.get::<_, Option<i64>>(3)?.map(|u| u as u32),
                folder_path: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                msgid: r.get::<_, Option<String>>(5)?,
                candidate_paths: Vec::new(),
            })
        })?;
        let mut out: Vec<PendingAction> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        for row in &mut out {
            if let Some(json) = payloads.get(&row.action_id) {
                row.payload_json = Arc::clone(json);
            }
        }
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
        // A move has one source: the placement it took the message out of.
        // The rows so far are one per placement the message still has, and
        // on a labels provider that is All Mail with a UID of its own, so the
        // row read "already in All Mail", the drain marked the archive
        // delivered, and the Inbox label stayed on the server — the phone
        // went on showing the conversation in its inbox. A move kind is
        // addressed at the queue-time source whenever that placement is
        // gone, and carries one row per message.
        let mut collapsed: Vec<PendingAction> = Vec::with_capacity(out.len());
        // Whether the last row pushed was addressed at its queue-time source.
        // Such a row is final: a later placement row for the same message is
        // a place the message still sits, not the place the change is about.
        let mut last_pinned = false;
        for row in out {
            let moves = matches!(
                serde_json::from_str::<crate::actions::ActionKind>(&row.kind_json),
                Ok(crate::actions::ActionKind::Archive
                    | crate::actions::ActionKind::Trash
                    | crate::actions::ActionKind::Spam
                    | crate::actions::ActionKind::Move)
            );
            if !moves {
                collapsed.push(row);
                last_pinned = false;
                continue;
            }
            let same_message = collapsed.last().is_some_and(|last| {
                last.action_id == row.action_id && last.message_id == row.message_id
            });
            if same_message {
                // A second placement of a message this action already
                // addresses. Keep whichever row has an address.
                if let Some(last) = collapsed.last_mut()
                    && !last_pinned
                    && last.uid.is_none()
                    && row.uid.is_some()
                {
                    *last = row;
                }
                continue;
            }
            let mut row = row;
            let source = serde_json::from_str::<crate::actions::ActionPayload>(&row.payload_json)
                .ok()
                .and_then(|payload| {
                    payload
                        .prior
                        .iter()
                        .find(|p| p.message_id == row.message_id)
                        .and_then(|p| Some((p.source_path.clone()?, p.source_uid)))
                });
            let mut pinned = false;
            if let Some((path, uid)) = source {
                let still_there: bool = self
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
                if !still_there {
                    // A source with no number is still the folder the change
                    // is about: the drain asks it for the number, and a
                    // folder that does not hold the message has nothing to
                    // change — which is an outcome, not a place to look next.
                    row.candidate_paths = if uid.is_none() {
                        vec![path.clone()]
                    } else {
                        Vec::new()
                    };
                    row.uid = uid;
                    row.folder_path = path;
                    pinned = true;
                }
            }
            collapsed.push(row);
            last_pinned = pinned;
        }
        Ok(collapsed)
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
        use crate::actions::{ActionKind, ActionPayload, ActionReceipt};

        // Refused here rather than defaulted to something plausible: a move with
        // no destination is a bug in the caller, and inventing one would file
        // mail somewhere nobody asked for.
        if kind.needs_target() && target.is_none() {
            return Err(StoreError::Rejected(format!(
                "{kind:?} needs a target folder or tag"
            )));
        }

        // And refused *before* anything is touched if the target is not this
        // account's to file into.
        //
        // Move clears every placement and then files the message in the
        // destination. There is no transaction around the pair, so a
        // destination that no longer exists failed on the insert with the
        // clearing already committed — leaving the message placed nowhere at
        // all: out of the inbox, out of the folder, out of every view, and
        // not in the trash either. A filter rule still naming a folder the
        // user has since deleted did this silently, to every message it
        // matched.
        //
        // Another account's folder is the same instruction wearing a
        // plausible id: it exists, the insert succeeds, and the mail is filed
        // next door. Ownership is the question worth asking, and it answers
        // the deleted case on the way past.
        //
        // Checked here rather than mended in the branch because the same
        // hazard belongs to every caller, and a target this account does not
        // have is not a move to fix up — it is a move that cannot be
        // performed.
        if let Some(id) = target {
            let unavailable = match kind {
                ActionKind::Move => !self.account_owns_folder(account_id, id)?,
                ActionKind::Tag | ActionKind::Untag => !self.account_owns_tag(account_id, id)?,
                // Snooze's target is an instant, not a row.
                _ => false,
            };
            if unavailable {
                return Err(StoreError::Rejected(format!(
                    "{kind:?} names a folder or tag this account does not have"
                )));
            }
        }

        let flag_filter = match kind {
            ActionKind::MarkRead => format!(" AND flags & {} = 0", flags::SEEN),
            ActionKind::MarkUnread => format!(" AND flags & {} != 0", flags::SEEN),
            _ => String::new(),
        };
        // Archive and Move take a message out of the inbox or a folder. They
        // do not take your own replies out of Sent, or pull a message out of
        // the bin: a thread-wide archive used to relocate every message in
        // the conversation, wherever it sat, and the drain then moved your
        // Sent copy to Archive on the server. Bins are exclusive whatever the
        // provider, so Trash and Spam still take everything.
        //
        // Where folders are labels, archiving is one thing: taking the Inbox
        // label off. Your own reply carries it too — Gmail puts a reply in
        // the conversation's inbox — and a reply exempted for sitting in
        // Sent kept its inbox placement, so the conversation stayed listed
        // here and stayed labelled there, and could not be archived at all.
        // So there the members an archive touches are exactly those holding
        // an inbox placement, Sent or not; the rest have nothing to lose and
        // are left alone, which is also what keeps the drain from moving a
        // Sent copy anywhere.
        let kept_where_they_are: Vec<i64> = match (kind, policy) {
            (ActionKind::Archive, crate::actions::PlacementPolicy::Labels) => {
                let mut stmt = self.conn.prepare(
                    "SELECT m.id FROM messages m
                     WHERE coalesce(m.thread_id, -m.id) = ?1 AND m.deleted_at_ms IS NULL
                       AND (EXISTS (SELECT 1 FROM placements p
                                    JOIN folders f ON f.id = p.folder_id
                                    WHERE p.message_id = m.id
                                      AND f.role IN ('drafts','trash','spam'))
                            OR NOT EXISTS (SELECT 1 FROM placements p
                                           JOIN folders f ON f.id = p.folder_id
                                           WHERE p.message_id = m.id AND f.role = 'inbox'))",
                )?;
                stmt.query_map(params![thread_id], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (ActionKind::Archive | ActionKind::Move, _) => {
                let mut stmt = self.conn.prepare(
                    "SELECT DISTINCT m.id FROM messages m
                     JOIN placements p ON p.message_id = m.id
                     JOIN folders f ON f.id = p.folder_id
                     WHERE coalesce(m.thread_id, -m.id) = ?1 AND m.deleted_at_ms IS NULL
                       AND f.role IN ('sent','drafts','trash','spam')",
                )?;
                stmt.query_map(params![thread_id], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            _ => Vec::new(),
        };
        let exclude = |column: &str| {
            if kept_where_they_are.is_empty() {
                String::new()
            } else {
                let list: Vec<String> = kept_where_they_are.iter().map(|i| i.to_string()).collect();
                format!(" AND {column} NOT IN ({})", list.join(","))
            }
        };
        let bare_filter = format!("{flag_filter}{}", exclude("id"));
        let aliased_filter = format!("{flag_filter}{}", exclude("m.id"));
        let ids: Vec<i64> = {
            let sql = format!(
                "SELECT id FROM messages
                 WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL{bare_filter}
                 ORDER BY id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            stmt.query_map(params![thread_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if ids.is_empty() {
            return Ok(ActionReceipt {
                action_id: 0,
                kind,
                message_count: 0,
                description: kind.past_tense().to_string(),
            });
        }

        // Captured *before* anything changes: undo restores what was, rather
        // than guessing an inverse — which breaks as soon as two actions touch
        // the same message. One pass per table, not one query per message —
        // a 22k-message mark_read used to hold the store lock for fifteen
        // seconds while status waited.
        // Where archiving is one label coming off, the change is about the
        // inbox placement whether or not it carries a number. Everywhere
        // else an archive is a move, and a move needs an address it can use.
        let address_at_inbox =
            matches!(kind, ActionKind::Archive) && !policy.archive_clears_everything();
        let prior =
            self.capture_thread_priors(thread_id, address_at_inbox, &bare_filter, &aliased_filter)?;

        // Queue rows before the local flag flip. The INSERT SELECT reuses the
        // same unread/read filter; if SEEN is already set, it would insert
        // nothing and the drain would never see the work.
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
        self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO action_messages(action_id, message_id)
                 SELECT ?1, id FROM messages
                 WHERE coalesce(thread_id, -id) = ?2 AND deleted_at_ms IS NULL{bare_filter}"
            ),
            params![action_id, thread_id],
        )?;

        match kind {
            ActionKind::Star => self.set_thread_flags(thread_id, flags::FLAGGED, 0)?,
            ActionKind::Unstar => self.set_thread_flags(thread_id, 0, flags::FLAGGED)?,
            ActionKind::MarkRead => self.set_thread_flags(thread_id, flags::SEEN, 0)?,
            ActionKind::MarkUnread => self.set_thread_flags(thread_id, 0, flags::SEEN)?,
            _ => {}
        }
        for id in &ids {
            match kind {
                ActionKind::Star
                | ActionKind::Unstar
                | ActionKind::MarkRead
                | ActionKind::MarkUnread => {}
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
                // Every exclusive move below clears the placements the
                // message had *other than* one already in the destination.
                // A member the user filed there earlier keeps that row, and
                // with it the UID the server knows it by; replacing the row
                // left the placement with no number, which the server could
                // neither flag nor prune, and which STATUS disagreed with
                // every cycle until a refetch healed it.
                ActionKind::Move => {
                    let dest = target.expect("checked above");
                    self.conn.execute(
                        "DELETE FROM placements WHERE message_id = ?1 AND folder_id != ?2",
                        params![id, dest],
                    )?;
                    self.place_message(*id, dest)?;
                }
                ActionKind::Archive => {
                    let dest = self.ensure_folder(account_id, "archive", "archive")?;
                    if policy.archive_clears_everything() {
                        self.conn.execute(
                            "DELETE FROM placements WHERE message_id = ?1 AND folder_id != ?2",
                            params![id, dest],
                        )?;
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
                    self.conn.execute(
                        "DELETE FROM placements WHERE message_id = ?1 AND folder_id != ?2",
                        params![id, dest],
                    )?;
                    self.place_message(*id, dest)?;
                }
            }
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
    ///
    /// Only the part of the snapshot the action touched: flags for a flag
    /// action, placements for a move, tags for a tag, the snooze for a snooze.
    /// Restoring the whole snapshot replayed it over whatever came later —
    /// mark read, archive, undo the mark-read, and the conversation walked
    /// back into the inbox, which a toast outliving the next action could do.
    pub fn undo_action(&self, action_id: i64) -> Result<bool> {
        use crate::actions::{ActionKind, ActionPayload};

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

        let flag_bit = match payload.kind {
            ActionKind::MarkRead | ActionKind::MarkUnread => Some(flags::SEEN),
            ActionKind::Star | ActionKind::Unstar => Some(flags::FLAGGED),
            _ => None,
        };
        let places = matches!(
            payload.kind,
            ActionKind::Archive | ActionKind::Trash | ActionKind::Spam | ActionKind::Move
        );
        let tags = matches!(payload.kind, ActionKind::Tag | ActionKind::Untag);
        let snooze = matches!(payload.kind, ActionKind::Snooze | ActionKind::Unsnooze);

        // One transaction: an undo that fails halfway used to leave the
        // placements cleared and nothing put back, with the action still
        // queued — the message in no folder at all, and the drain about to
        // deliver the move the user had just cancelled.
        let tx = self.conn.unchecked_transaction()?;
        for p in &payload.prior {
            if let Some(bit) = flag_bit {
                // The one bit, as it was: a star set since survives undoing a
                // mark-read, and the other way round.
                tx.execute(
                    "UPDATE messages SET flags = (flags & ~?2) | (?3 & ?2) WHERE id = ?1",
                    params![p.message_id, bit, p.flags],
                )?;
            }
            if !places {
                if tags {
                    self.restore_tags(p)?;
                }
                if snooze {
                    tx.execute(
                        "UPDATE messages SET snoozed_until_ms = ?2 WHERE id = ?1",
                        params![p.message_id, p.snoozed_until],
                    )?;
                }
                continue;
            }
            // Only into folders that still exist. A folder the survey has
            // since forgotten cannot take a placement, and a message that
            // can be put back nowhere keeps the placement it has rather than
            // being left with none.
            let mut restorable: Vec<i64> = Vec::with_capacity(p.folder_ids.len());
            for f in &p.folder_ids {
                let exists: bool = tx
                    .query_row("SELECT 1 FROM folders WHERE id = ?1", params![f], |_| {
                        Ok(true)
                    })
                    .optional()?
                    .unwrap_or(false);
                if exists {
                    restorable.push(*f);
                }
            }
            if restorable.is_empty() {
                continue;
            }
            tx.execute(
                "DELETE FROM placements WHERE message_id = ?1",
                params![p.message_id],
            )?;
            for f in &restorable {
                // With its UID where the UID is known. Restored without one,
                // the placement could not take a flag change from the server,
                // was never pruned when the server dropped the message, and
                // disagreed with STATUS every cycle until a refetch healed it.
                match (p.source_folder, p.source_uid) {
                    (Some(folder), Some(uid)) if folder == *f => {
                        self.place_message_at(p.message_id, *f, uid)?
                    }
                    _ => self.place_message(p.message_id, *f)?,
                }
            }
        }

        tx.execute(
            "UPDATE actions SET state = 'undone' WHERE id = ?1",
            params![action_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Same shape as folders: wipe and restore what was captured, rather than
    /// removing whatever the action added. Those differ whenever the tag was
    /// already on the message before the action ran.
    fn restore_tags(&self, p: &crate::actions::PriorState) -> Result<()> {
        self.conn.execute(
            "DELETE FROM message_tags WHERE message_id = ?1",
            params![p.message_id],
        )?;
        for tag in &p.tag_ids {
            self.tag_message(p.message_id, *tag)?;
        }
        Ok(())
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
    /// Counts one failed delivery, and reports how many there have been.
    ///
    /// The column existed and nothing ever wrote to it, so an action that
    /// could never be delivered was retried on every sync cycle for as long as
    /// the app ran — one in a real mailbox had failed 112 times and was still
    /// going. Counting is what lets the drain decide it has asked enough.
    pub fn record_attempt(&self, action_id: i64) -> Result<i64> {
        self.conn.execute(
            "UPDATE actions SET attempts = attempts + 1 WHERE id = ?1",
            params![action_id],
        )?;
        Ok(self.conn.query_row(
            "SELECT attempts FROM actions WHERE id = ?1",
            params![action_id],
            |r| r.get(0),
        )?)
    }

    /// Only a queued action moves. The drain snapshots the queue once and
    /// delivers from the snapshot, so an action undone while a cycle was in
    /// flight still arrives here a moment later — and 'undone' overwritten
    /// with 'sent' is a change the user cancelled, recorded as delivered.
    /// What is undone stays undone; the drain checks `action_state` before
    /// each item as well, and this is the belt under that.
    pub fn mark_action_state(&self, action_id: i64, state: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE actions SET state = ?2 WHERE id = ?1 AND state = 'queued'",
            params![action_id, state],
        )?;
        Ok(())
    }

    /// The state an action is in, or None for an id that never existed.
    pub fn action_state(&self, action_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM actions WHERE id = ?1",
                params![action_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Records the outcome of one message of an action: it reached the server
    /// (`delivered`), or there was no server copy for it to change and
    /// asking again cannot learn more (dropped). The message leaves the queue
    /// either way; the action settles only when every message has an
    /// outcome — sent if any reached the server, undeliverable if none did.
    /// Returns whether the action settled on this call.
    ///
    /// Per message because an action carries several: an archive of a
    /// three-message conversation is three MOVEs, and settling the action on
    /// the first success discarded the other two.
    pub fn mark_message_outcome(
        &self,
        action_id: i64,
        message_id: i64,
        delivered: bool,
    ) -> Result<bool> {
        // An action that is no longer queued has nothing to settle: undone
        // between the drain's snapshot and its delivery, it stays undone,
        // and its rows keep no record of a delivery the user cancelled.
        if self.action_state(action_id)?.as_deref() != Some("queued") {
            return Ok(false);
        }
        let column = if delivered {
            "delivered_ms"
        } else {
            "dropped_ms"
        };
        self.conn.execute(
            &format!(
                "UPDATE action_messages SET {column} = (strftime('%s','now') * 1000)
                 WHERE action_id = ?1 AND message_id = ?2 AND {column} IS NULL"
            ),
            params![action_id, message_id],
        )?;
        let open: i64 = self.conn.query_row(
            "SELECT count(*) FROM action_messages
             WHERE action_id = ?1 AND delivered_ms IS NULL AND dropped_ms IS NULL",
            params![action_id],
            |r| r.get(0),
        )?;
        if open > 0 {
            return Ok(false);
        }
        let reached: i64 = self.conn.query_row(
            "SELECT count(*) FROM action_messages
             WHERE action_id = ?1 AND delivered_ms IS NOT NULL",
            params![action_id],
            |r| r.get(0),
        )?;
        self.mark_action_state(
            action_id,
            if reached > 0 { "sent" } else { "undeliverable" },
        )?;
        Ok(true)
    }

    /// Priors for every message an action will touch, in one pass per table.
    fn capture_thread_priors(
        &self,
        thread_id: i64,
        address_at_inbox: bool,
        bare_filter: &str,
        aliased_filter: &str,
    ) -> Result<Vec<crate::actions::PriorState>> {
        use crate::actions::PriorState;

        let mut by_id: HashMap<i64, PriorState> = HashMap::new();
        let mut order: Vec<i64> = Vec::new();
        {
            let sql = format!(
                "SELECT id, flags, snoozed_until_ms FROM messages
                 WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL{bare_filter}
                 ORDER BY id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![thread_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?;
            for row in rows {
                let (id, flags, snoozed_until) = row?;
                order.push(id);
                by_id.insert(
                    id,
                    PriorState {
                        message_id: id,
                        flags,
                        folder_ids: Vec::new(),
                        tag_ids: Vec::new(),
                        source_path: None,
                        source_uid: None,
                        source_folder: None,
                        snoozed_until,
                    },
                );
            }
        }
        {
            let sql = format!(
                "SELECT mt.message_id, mt.tag_id FROM message_tags mt
                 JOIN messages m ON m.id = mt.message_id
                 WHERE coalesce(m.thread_id, -m.id) = ?1 AND m.deleted_at_ms IS NULL{aliased_filter}"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![thread_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, tag) = row?;
                if let Some(p) = by_id.get_mut(&id) {
                    p.tag_ids.push(tag);
                }
            }
        }
        {
            let sql = format!(
                "SELECT p.message_id, p.folder_id FROM placements p
                 JOIN messages m ON m.id = p.message_id
                 WHERE coalesce(m.thread_id, -m.id) = ?1 AND m.deleted_at_ms IS NULL{aliased_filter}"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![thread_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, folder) = row?;
                if let Some(p) = by_id.get_mut(&id) {
                    p.folder_ids.push(folder);
                }
            }
        }
        {
            // Inbox first so a message in two folders keeps the same address
            // the per-row query used to pick.
            //
            // A labels archive is addressed at the inbox placement whether
            // or not that placement has a number: the inbox is what the
            // archive takes the message out of, and a placement the label
            // sweep made has none. Addressed at the next best placement
            // instead — the Sent copy, say — the drain moved that copy on
            // the server, which is a different change from the one that was
            // asked for. With no number the drain asks the inbox which of
            // its numbers carries the Message-ID, as it does after a
            // renumbering. Every other action wants an address it can use at
            // once, so those keep to numbered placements.
            let addressed_only = if address_at_inbox {
                ""
            } else {
                " AND p.uid IS NOT NULL"
            };
            let sql = format!(
                "SELECT p.message_id, f.path, p.uid, (f.role = 'inbox'), p.folder_id
                 FROM placements p
                 JOIN folders f ON f.id = p.folder_id
                 JOIN messages m ON m.id = p.message_id
                 WHERE coalesce(m.thread_id, -m.id) = ?1 AND m.deleted_at_ms IS NULL
                   {addressed_only}{aliased_filter}
                 ORDER BY p.message_id, (f.role = 'inbox') DESC, (p.uid IS NOT NULL) DESC"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![thread_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, i64>(4)?,
                ))
            })?;
            for row in rows {
                let (id, path, uid, folder) = row?;
                if let Some(p) = by_id.get_mut(&id)
                    && p.source_path.is_none()
                {
                    p.source_path = Some(path);
                    p.source_uid = uid.map(|u| u as u32);
                    p.source_folder = Some(folder);
                }
            }
        }
        Ok(order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect())
    }

    fn set_thread_flags(&self, thread_id: i64, add: i64, remove: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET flags = (flags | ?2) & ~?3
             WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL",
            params![thread_id, add, remove],
        )?;
        Ok(())
    }

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
    /// A tag somebody asked for. Never removed on its own, empty or not.
    pub fn ensure_tag(&self, account_id: i64, name: &str, colour: Option<&str>) -> Result<i64> {
        self.ensure_tag_from(account_id, name, colour, "user")
    }

    /// A tag that exists only because a keyword arrived on a message.
    ///
    /// Recorded as the server's so `forget_abandoned_server_tags` can clear it
    /// up once nothing carries the keyword any more. A tag somebody applies by
    /// hand here afterwards is promoted to theirs by `ensure_tag`, because the
    /// ON CONFLICT below writes the origin again.
    pub fn ensure_server_tag(&self, account_id: i64, name: &str) -> Result<i64> {
        self.ensure_tag_from(account_id, name, None, "server")
    }

    fn ensure_tag_from(
        &self,
        account_id: i64,
        name: &str,
        colour: Option<&str>,
        origin: &str,
    ) -> Result<i64> {
        // Matched without regard to case, because that is what a tag is here.
        // `rename_tag` already refuses a name that differs only in case, and
        // the UNIQUE(account_id, name) constraint this leant on compares with
        // SQLite's BINARY collation — so `Urgent` and `urgent` were two rows:
        // a state reachable by creating one, unreachable by renaming one, and
        // impossible for the server to hold at all, since IMAP keywords are
        // case-insensitive and both travel as the same keyword.
        //
        // The spelling already stored wins. A keyword coming back as `URGENT`
        // from another client is the tag you named `Urgent`, not an
        // instruction to relabel it.
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM tags WHERE account_id = ?1 AND lower(name) = lower(?2)",
                params![account_id, name],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.conn.execute(
                "UPDATE tags SET
                     colour = coalesce(?2, colour),
                     -- Only ever upgrades. A tag the server introduced and a
                     -- person then used is theirs; the reverse is not true, or
                     -- every sync would hand their tags back to the server.
                     origin = CASE WHEN ?3 = 'user' THEN 'user' ELSE origin END
                 WHERE id = ?1",
                params![id, colour, origin],
            )?;
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO tags(account_id, name, colour, origin) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, name) DO UPDATE SET
                 colour = coalesce(excluded.colour, colour),
                 origin = CASE WHEN excluded.origin = 'user' THEN 'user' ELSE origin END",
            params![account_id, name, colour, origin],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM tags WHERE account_id = ?1 AND name = ?2",
            params![account_id, name],
            |r| r.get(0),
        )?)
    }

    /// Drops tags that only ever existed because the server mentioned them and
    /// that now label nothing.
    ///
    /// The other half of promoting a keyword into a sidebar entry. Without it,
    /// a keyword that arrives once and later goes — the message deleted, or
    /// untagged in another client — leaves a tag behind for good. That is how
    /// a live account ended up with an empty "Followup" nobody had made.
    ///
    /// Deliberately narrow. Only `origin = 'server'` rows are eligible, so a
    /// tag somebody created, or one they adopted by applying it themselves, is
    /// never touched however empty it is: an empty tag of your own is a
    /// waiting label, not litter. Tags predating the origin column count as
    /// theirs, which is the answer that cannot delete something wanted.
    pub fn forget_abandoned_server_tags(&self, account_id: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM tags
             WHERE account_id = ?1
               AND origin = 'server'
               AND id NOT IN (SELECT tag_id FROM message_tags)",
            params![account_id],
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
    /// Whether this account has that tag. The same story as
    /// `account_owns_folder`: a rule keeps naming a tag long after the tag
    /// has gone, and an id says nothing about whose it was.
    pub fn account_owns_tag(&self, account_id: i64, tag_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tags WHERE id = ?1 AND account_id = ?2",
                params![tag_id, account_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

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
                  -- The count follows the view: a conversation in the bin is
                  -- not still Urgent, and the rail said 1 over an empty list.
                  AND NOT EXISTS (SELECT 1 FROM placements p
                                  JOIN folders f ON f.id = p.folder_id
                                  WHERE p.message_id = m.id
                                    AND f.role IN ('trash','spam'))
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
                    // A keyword nobody here made a tag for. Machine keywords
                    // stay flags rather than becoming sidebar entries; see
                    // keywords::is_system_keyword.
                    None if crate::keywords::is_system_keyword(kw) => continue,
                    None => self.ensure_server_tag(account_id, kw)?,
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
        // Here rather than on a timer: this is the only place a keyword stops
        // being carried, so it is the only moment a server tag can become
        // abandoned. Doing it now means the sidebar never shows an orphan, not
        // even until the next restart.
        self.forget_abandoned_server_tags(account_id)?;
        Ok(changed)
    }

    pub fn apply_gmail_labels(
        &self,
        account_id: i64,
        labelled: &[(String, Vec<String>)],
    ) -> Result<usize> {
        self.file_by_gmail_labels(
            account_id,
            labelled
                .iter()
                .map(|(m, l)| (m.as_str(), l.as_slice(), None)),
        )
    }

    /// `apply_gmail_labels`, with each message's UID in INBOX where the
    /// caller knows it.
    ///
    /// A sweep over All Mail learns All Mail's numbers, and the inbox
    /// placement it makes carries none: the inbox sweep can never prune it,
    /// a flag change from the server cannot find it, and an archive of it
    /// has to ask the server for the number first. Given the number — from
    /// a listing of INBOX's `(UID, Message-ID)` pairs, the way the All Mail
    /// walk lists All Mail — the placement is as good as a fetched one. A
    /// number already held is kept when none is given.
    pub fn apply_gmail_labels_at(
        &self,
        account_id: i64,
        labelled: &[(String, Vec<String>, Option<u32>)],
    ) -> Result<usize> {
        self.file_by_gmail_labels(
            account_id,
            labelled
                .iter()
                .map(|(m, l, uid)| (m.as_str(), l.as_slice(), *uid)),
        )
    }

    fn file_by_gmail_labels<'a>(
        &self,
        account_id: i64,
        labelled: impl Iterator<Item = (&'a str, &'a [String], Option<u32>)>,
    ) -> Result<usize> {
        // The label arrives quoted and how many backslashes survive is a detail
        // of the parser, so match on the name rather than the escaping — and
        // on the whole name. A suffix match kept every message under a user
        // label such as "Old Inbox" in the inbox after archiving, and starred
        // anything under one ending in "Starred".
        let has = |ls: &[String], name: &str| {
            ls.iter()
                .any(|l| l.trim_matches('"').trim_start_matches('\\') == name)
        };

        let inbox = self.ensure_folder(account_id, "inbox", "INBOX")?;
        let archive = self.ensure_folder(account_id, "archive", "archive")?;
        let tags: Vec<(String, i64)> = self
            .tags_for_account(account_id)?
            .into_iter()
            .map(|t| (t.name, t.id))
            .collect();
        let mut changed = 0usize;

        for (msg_id, labels, inbox_uid) in labelled {
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
            match inbox_uid {
                Some(uid) if in_inbox => self.place_message_at(id, add, uid)?,
                _ => self.place_message(id, add)?,
            }

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
