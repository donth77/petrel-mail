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
        let identity = self.identity(account_id)?;

        let id = match draft_id {
            Some(id) => {
                self.conn.execute(
                    "UPDATE messages
                     SET date_ms = ?2, subject = ?3, snippet = ?4, draft_body = ?5,
                         draft_html = ?6, draft_envelope = ?7
                     WHERE id = ?1",
                    params![id, now, subject, snippet, body, html, envelope_json],
                )?;
                id
            }
            None => {
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
        self.conn
            .execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        Ok(())
    }
}
