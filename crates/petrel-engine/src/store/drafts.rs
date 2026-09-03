//! Drafts and the outbox: mail that is still yours, and mail on its way
//! out — the two states where losing bytes is unforgivable.
//!
//! Moved verbatim from mod.rs (Phase 1.5).
use super::*;

impl Store {
    /// Saves a draft, or updates one already saved.
    ///
    /// Stored as an ordinary message row carrying the \Draft flag and placed in
    /// the drafts folder, rather than in a table of its own. That is what makes
    /// the Drafts view, search, and every triage action work on drafts without
    /// any of them learning a second kind of thing — and it is how a draft
    /// reaches the server the day sync learns to APPEND one.
    /// The Message-ID header a stored message carries, for a reply that
    /// must thread into its conversation at the other end.
    /// A server revision of this draft that is not the copy this store
    /// pushed: a second-copy row sharing the draft's Message-ID, standing in
    /// the drafts folder. The reconcile sweep creates exactly this shape when
    /// another client saved its own version of the draft, so its presence is
    /// the conflict — no network question needed at composer-open time.
    pub fn draft_conflict(&self, draft_id: i64) -> Result<Option<(i64, Option<i64>)>> {
        let (msgid, _) = self.draft_sync_state(draft_id)?;
        let Some(msgid) = msgid else {
            return Ok(None);
        };
        Ok(self
            .conn
            .query_row(
                "SELECT m.id, p.uid FROM messages m
                 JOIN placements p ON p.message_id = m.id
                 JOIN folders f ON f.id = p.folder_id AND f.role = 'drafts'
                 WHERE m.message_id_hdr LIKE ?1 || '::copy-%'
                   AND m.deleted_at_ms IS NULL
                 ORDER BY p.uid DESC LIMIT 1",
                params![msgid],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?)
    }

    /// Makes the server's revision the draft: its words and subject land in
    /// the draft columns, its UID becomes the recorded one, and the next
    /// composer open shows what was chosen.
    pub fn adopt_server_revision(
        &self,
        draft_id: i64,
        subject: &str,
        body: &str,
        html: &str,
        uid: Option<u32>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET subject = ?2, draft_body = ?3, draft_html = ?4,
                    draft_server_uid = ?5
             WHERE id = ?1",
            params![draft_id, subject, body, html, uid.map(|u| u as i64)],
        )?;
        Ok(())
    }

    /// Removes the second-copy row a resolved conflict leaves behind. The
    /// blob is content-addressed and may be shared, so only the row and its
    /// placements go; gc owns the bytes.
    pub fn retire_second_copy(&self, message_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM placements WHERE message_id = ?1",
            params![message_id],
        )?;
        self.conn
            .execute("DELETE FROM messages WHERE id = ?1", params![message_id])?;
        Ok(())
    }

    pub fn msgid_header_of(&self, message_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT message_id_hdr FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Records what the reader answered to an invitation.
    pub fn set_invite_response(&self, message_id: i64, response: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET invite_response = ?2 WHERE id = ?1",
            params![message_id, response],
        )?;
        Ok(())
    }

    pub fn save_draft(
        &self,
        account_id: i64,
        draft_id: Option<i64>,
        to: &str,
        subject: &str,
        body: &str,
        html: &str,
    ) -> Result<i64> {
        self.save_draft_full(
            account_id,
            draft_id,
            to,
            "",
            subject,
            body,
            html,
            &DraftEnvelope::default(),
        )
    }

    /// The draft's server identity: its stable Message-ID and the UID of the
    /// copy currently in the server's Drafts folder.
    /// The account a message belongs to, tombstoned or not. The callers that
    /// push or drop a draft's server copy used to ask for the *active*
    /// account instead, and with two accounts that is whichever one the rail
    /// happened to show — a draft written in one account was expunged from
    /// the other's Drafts.
    pub fn account_of_message(&self, message_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT account_id FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn draft_sync_state(&self, draft_id: i64) -> Result<(Option<String>, Option<u32>)> {
        Ok(self
            .conn
            .query_row(
                "SELECT draft_msgid, draft_server_uid FROM messages WHERE id = ?1",
                params![draft_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<i64>>(1)?.map(|u| u as u32),
                    ))
                },
            )
            .optional()?
            .unwrap_or((None, None)))
    }

    /// Gives a draft its travelling name, once, for life.
    ///
    /// Also written as the dedupe key: the server copy comes back through
    /// ordinary folder sync, and carrying the same Message-ID is what makes
    /// it land on this row — an edit of the draft, not a sibling beside it.
    pub fn set_draft_msgid(&mut self, draft_id: i64, msgid: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET draft_msgid = ?2, message_id_hdr = ?2 WHERE id = ?1",
            params![draft_id, msgid],
        )?;
        Ok(())
    }

    /// Records (or clears) which server UID currently holds this draft.
    pub fn set_draft_server_uid(&mut self, draft_id: i64, uid: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET draft_server_uid = ?2 WHERE id = ?1",
            params![draft_id, uid.map(|u| u as i64)],
        )?;
        Ok(())
    }

    /// Saves a draft with everything it needs to go out, not only its text.
    #[allow(clippy::too_many_arguments)]
    pub fn save_draft_full(
        &self,
        account_id: i64,
        draft_id: Option<i64>,
        to: &str,
        cc: &str,
        subject: &str,
        body: &str,
        html: &str,
        envelope: &DraftEnvelope,
    ) -> Result<i64> {
        let envelope_json = serde_json::to_string(envelope).unwrap_or_else(|_| "{}".into());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // The list shows a snippet; an empty draft still needs to be findable,
        // so it gets a placeholder rather than a blank row.
        let snippet: String = body.chars().take(200).collect();

        // A draft that exists belongs to the account it was written in, not
        // to whichever account the window happens to show. The composer
        // follows an account switch, and a save under the new account used
        // to file the draft's placement in the other account's Drafts.
        //
        // And a draft that no longer exists — discarded, or sent, with an
        // autosave still in flight — is refused rather than re-indexed: the
        // index row was written first, the message row never came back, and
        // the orphan then failed every search that touched its words.
        let account_id = match draft_id {
            Some(id) => self
                .account_of_message(id)?
                .ok_or_else(|| StoreError::Rejected("that draft no longer exists".into()))?,
            None => account_id,
        };

        let id = match draft_id {
            Some(id) => {
                let n = self.conn.execute(
                    "UPDATE messages
                     SET date_ms = ?2, subject = ?3, snippet = ?4, draft_body = ?5,
                         draft_html = ?6, draft_envelope = ?7
                     WHERE id = ?1",
                    params![id, now, subject, snippet, body, html, envelope_json],
                )?;
                if n == 0 {
                    return Err(StoreError::Rejected("that draft no longer exists".into()));
                }
                id
            }
            None => {
                let identity = self.identity(account_id)?;
                self.conn.execute(
                    "INSERT INTO messages(account_id, date_ms, from_addr, from_display,
                                          subject, snippet, draft_body, draft_html, flags,
                                          draft_envelope)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        account_id,
                        now,
                        identity.address,
                        identity.display_name,
                        subject,
                        snippet,
                        body,
                        html,
                        flags::DRAFT | flags::SEEN,
                        envelope_json
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
        };

        // Searchable, as the module doc has always promised: the text goes into
        // fts_content like any other message's. Until it did, a draft could
        // only be found by `in:drafts`, never by a word in it.
        self.conn.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             VALUES (?1, ?2, ?3, ?4, '')
             ON CONFLICT(message_id) DO UPDATE SET
                subject = excluded.subject, body_text = excluded.body_text,
                addrs = excluded.addrs",
            params![id, subject, body, format!("{to} {cc}")],
        )?;

        // Recipients live where every other message keeps them, so the list can
        // show who a draft is to without a special case.
        self.conn.execute(
            "DELETE FROM message_addresses WHERE message_id = ?1",
            params![id],
        )?;
        for (role, list) in [("to", to), ("cc", cc)] {
            for addr in list
                .split([',', ';'])
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                self.conn.execute(
                    "INSERT INTO message_addresses(message_id, role, addr_norm, display)
                     VALUES (?1, ?2, ?3, ?3)",
                    params![id, role, addr],
                )?;
            }
        }

        let folder = self.ensure_folder(account_id, "drafts", "drafts")?;
        self.conn
            .execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
        self.place_message(id, folder)?;
        Ok(id)
    }

    /// Marks a draft to go at a given time, or clears the schedule.
    ///
    /// Clearing matters as much as setting: an outbox you cannot pull something
    /// back out of is a worse promise than sending straight away, because the
    /// window where you can change your mind is exactly why it exists.
    pub fn schedule_send(&self, draft_id: i64, at_ms: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET send_after_ms = ?2 WHERE id = ?1",
            params![draft_id, at_ms],
        )?;
        Ok(())
    }

    /// Drafts whose time has come.
    ///
    /// A comparison against the clock, not a timer — so a message due while the
    /// app was closed goes out on the next pass instead of being missed by an
    /// alarm that never rang.
    /// Messages whose turn it is to go.
    ///
    /// Two conditions, and the second is the one that matters: the scheduled
    /// time has passed *and* the message is in a state that may be sent on its
    /// own. One held for a person — whose outcome could not be proved either
    /// way — is never picked up here however long it waits. That is the whole
    /// ambiguous-outcome rule: a retry the engine cannot prove safe is a
    /// decision, and decisions are handed over rather than made.
    ///
    /// `send_next_ms` is the retry ladder's next rung; a freshly scheduled
    /// message has none and goes on `send_after_ms` alone.
    pub fn due_sends(&self, account_id: i64, now_ms: i64) -> Result<Vec<DraftRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM messages
             WHERE account_id = ?1
               AND send_after_ms IS NOT NULL AND send_after_ms <= ?2
               AND coalesce(send_next_ms, 0) <= ?2
               AND coalesce(send_state, 'RetryQueued') IN ('UndoWindow', 'RetryQueued')
             ORDER BY send_after_ms",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(params![account_id, now_ms], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter().map(|id| self.load_draft(id)).collect()
    }

    /// When the next outbox message becomes due, if any is waiting.
    ///
    /// The instant a clock should wake at: the earliest of each sendable
    /// message's scheduled time and its retry time, whichever is later for
    /// that message. Held messages do not count — they have no time, they
    /// have a person.
    pub fn next_due_ms(&self, account_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT min(max(send_after_ms, coalesce(send_next_ms, 0)))
                   FROM messages
                  WHERE account_id = ?1 AND send_after_ms IS NOT NULL
                    AND coalesce(send_state, 'RetryQueued') IN ('UndoWindow', 'RetryQueued')",
                [account_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Records where a send attempt left a message.
    ///
    /// One call for every transition, so the five columns that describe an
    /// outbox row can never disagree with each other: a state of `Sent` with an
    /// error attached, or a retry time on a message held for a person, would be
    /// a row that says two things at once.
    pub fn set_send_state(
        &self,
        id: i64,
        state: crate::outbox::SendState,
        error: Option<&str>,
        next_ms: Option<i64>,
        message_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE messages
                SET send_state = ?2,
                    send_error = ?3,
                    send_next_ms = ?4,
                    send_message_id = coalesce(?5, send_message_id),
                    send_attempts = send_attempts + CASE WHEN ?6 THEN 1 ELSE 0 END
              WHERE id = ?1",
            params![
                id,
                format!("{state:?}"),
                error,
                next_ms,
                message_id,
                // An attempt is something that reached the wire. Being held,
                // or merely re-queued by hand, is not one.
                matches!(
                    state,
                    crate::outbox::SendState::Sent
                        | crate::outbox::SendState::RetryQueued
                        | crate::outbox::SendState::FailedPermanent
                        | crate::outbox::SendState::NeedsAttention
                ),
            ],
        )?;
        Ok(())
    }

    /// The Message-ID an outbox row's last attempt went out under, if any.
    pub fn conn_query_send_message_id(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT send_message_id FROM messages WHERE id = ?1",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// A send that died while `Transmitting` is held for a person.
    ///
    /// `Transmitting` is not on `due_sends`'s allow-list, and the outbox row
    /// offers no button for it, so a crash mid-SMTP left the message stuck
    /// forever with no way out. `NeedsAttention` is the honest leftover:
    /// we do not know whether the server accepted it.
    pub fn recover_interrupted_sends(&self) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE messages
                SET send_state = 'NeedsAttention',
                    send_error = 'interrupted before Petrel heard back'
              WHERE send_after_ms IS NOT NULL
                AND send_state = 'Transmitting'",
            [],
        )?;
        Ok(n)
    }

    /// Puts a message back on the queue to go at once, whatever state it was
    /// in. This is "Send now", "Try now" and "Send anyway": the person has
    /// looked and decided, which is the only thing that may move a message out
    /// of `NeedsAttention`.
    pub fn resend_now(&self, id: i64, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages
                SET send_state = 'RetryQueued', send_error = NULL,
                    send_next_ms = NULL, send_after_ms = ?2
              WHERE id = ?1",
            params![id, now_ms],
        )?;
        Ok(())
    }

    /// Takes a message out of the outbox and back into Drafts, keeping its
    /// text. "Edit" on a failed send: the message is not lost, it is yours
    /// again.
    pub fn unschedule_send(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages
                SET send_after_ms = NULL, send_state = NULL, send_error = NULL,
                    send_next_ms = NULL, send_attempts = 0
              WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// The outbox, with each row's state spelled out for the UI.
    pub fn outbox(&self, account_id: i64) -> Result<Vec<OutboxRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, coalesce(m.subject,''), m.send_after_ms,
                    coalesce(m.send_state, 'RetryQueued'), m.send_error,
                    m.send_attempts, m.send_next_ms,
                    (SELECT count(*) FROM attachments a WHERE a.message_id = m.id),
                    (SELECT group_concat(addr_norm, ', ') FROM message_addresses
                      WHERE message_id = m.id AND role = 'to')
             FROM messages m
             WHERE m.account_id = ?1 AND m.send_after_ms IS NOT NULL
             ORDER BY m.send_after_ms",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok(OutboxRow {
                id: r.get(0)?,
                subject: r.get(1)?,
                send_after_ms: r.get(2)?,
                state: r.get(3)?,
                error: r.get(4)?,
                attempts: r.get(5)?,
                next_ms: r.get(6)?,
                attachments: r.get(7)?,
                to: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Reads a draft back for editing.
    pub fn load_draft(&self, id: i64) -> Result<DraftRecord> {
        let (subject, body, html, envelope_json): (String, String, String, Option<String>) =
            self.conn.query_row(
                "SELECT coalesce(subject,''), coalesce(draft_body,''), coalesce(draft_html,''),
                        draft_envelope
                 FROM messages WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        let envelope = envelope_json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let addresses = |role: &str| -> Result<Vec<String>> {
            let mut stmt = self.conn.prepare(
                "SELECT addr_norm FROM message_addresses WHERE message_id = ?1 AND role = ?2",
            )?;
            let v = stmt
                .query_map(params![id, role], |r| r.get(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?;
            Ok(v)
        };
        let cc = addresses("cc")?;
        let mut stmt = self.conn.prepare(
            "SELECT addr_norm FROM message_addresses WHERE message_id = ?1 AND role = 'to'",
        )?;
        let to: Vec<String> = stmt
            .query_map(params![id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(DraftRecord {
            id,
            to: to.join(", "),
            cc: cc.join(", "),
            subject,
            body,
            html,
            envelope,
        })
    }

    /// Removes a draft once it has been sent or discarded.
    pub fn delete_draft(&self, id: i64) -> Result<()> {
        // The index row goes with it, or a sent draft's words keep matching.
        self.conn
            .execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        Ok(())
    }
}
