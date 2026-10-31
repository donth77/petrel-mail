//! Drafts and the outbox: mail that is still yours, and mail on its way
//! out — the two states where losing bytes is unforgivable.
//!
//! Moved verbatim from mod.rs (Phase 1.5).
use super::*;

impl Store {
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
        // Either suffix a second row under the draft's Message-ID can carry:
        // `::copy-N` for a second copy the reconcile met, `::b-…` for one keyed
        // by its content. Matched as exact prefixes rather than with LIKE, in
        // which a `_` or `%` in the Message-ID itself is a wildcard.
        Ok(self
            .conn
            .query_row(
                "SELECT m.id, p.uid FROM messages m
                 JOIN placements p ON p.message_id = m.id
                 JOIN folders f ON f.id = p.folder_id AND f.role = 'drafts'
                 WHERE ((m.message_id_hdr >= ?1 || '::copy-' AND m.message_id_hdr < ?1 || '::copy.')
                     OR (m.message_id_hdr >= ?1 || '::b-' AND m.message_id_hdr < ?1 || '::b.'))
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
    ///
    /// The search entry and the snippet move with the words, in the same unit
    /// of writes, as a save writes them. Before, the draft kept answering
    /// searches for the words it no longer had until it was next saved
    /// (docs/24 #33, docs/25 review).
    pub fn adopt_server_revision(
        &self,
        draft_id: i64,
        subject: &str,
        body: &str,
        html: &str,
        uid: Option<u32>,
    ) -> Result<()> {
        let snippet: String = body.chars().take(200).collect();
        let unit = self.atomic()?;
        let n = self.conn.execute(
            "UPDATE messages SET subject = ?2, draft_body = ?3, draft_html = ?4,
                    draft_server_uid = ?5, snippet = ?6
             WHERE id = ?1",
            params![
                draft_id,
                subject,
                body,
                html,
                uid.map(|u| u as i64),
                snippet
            ],
        )?;
        // Sent or discarded between the command's check and this write: no
        // row, so no index entry either. Written anyway, it was an entry
        // search returned for a message that is not there (docs/25 review),
        // which `save_draft_full` refuses for the same reason.
        if n == 0 {
            return Err(StoreError::Rejected("that draft no longer exists".into()));
        }
        self.conn.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             SELECT ?1, ?2, ?3,
                    coalesce((SELECT group_concat(a.addr_norm, ' ') FROM message_addresses a
                               WHERE a.message_id = ?1 AND a.role IN ('to', 'cc')), ''),
                    ''
             ON CONFLICT(message_id) DO UPDATE SET
                subject = excluded.subject, body_text = excluded.body_text,
                addrs = excluded.addrs",
            params![draft_id, subject, body],
        )?;
        unit.done()
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

    /// The Message-ID header a stored message carries, for a reply that
    /// must thread into its conversation at the other end: the wire id, never
    /// the store's own suffixes or the stand-in for a message that had none
    /// (`wire_message_id`).
    pub fn msgid_header_of(&self, message_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT message_id_hdr FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .and_then(|key| super::wire_message_id(&key).map(str::to_string)))
    }

    /// Records what the reader answered to an invitation.
    pub fn set_invite_response(&self, message_id: i64, response: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET invite_response = ?2 WHERE id = ?1",
            params![message_id, response],
        )?;
        Ok(())
    }

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

    /// Whether `id` is still a message this app is sending or wrote: one in
    /// the outbox, or a draft saved here.
    ///
    /// Undo, Edit and Discard act on an id they were handed earlier. Before
    /// schema step 28, SQLite gave the next row the highest id once the row
    /// that held it was deleted, so a message sent and removed could have its
    /// id taken a moment later by mail arriving, or by the Sent copy of
    /// itself, and an Undo or a Discard held from before acted on the
    /// newcomer. Ids are no longer given out twice, and this stays as the
    /// second guard: only a draft saved here has an envelope, and received
    /// mail never does.
    pub fn is_own_outgoing(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT draft_envelope IS NOT NULL OR send_after_ms IS NOT NULL
                   FROM messages WHERE id = ?1",
                params![id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
    }

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

    /// Where the server holds copies of a draft: `(folder path, UID)` for
    /// each numbered placement in Drafts, or in the Trash or Spam that a
    /// conversation took it to.
    ///
    /// What is dropped once the draft has gone or been discarded. Only Drafts
    /// was looked in, so a reply binned with its conversation left its copy in
    /// the bin, and it synced back as a draft there. A placement with no UID
    /// is not addressed: none was learned, or a UIDVALIDITY reset took it.
    pub fn draft_copies(&self, draft_id: i64) -> Result<Vec<(String, u32)>> {
        // In the draft's own account only: the copies are expunged by
        // signing in to that account.
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.uid FROM placements p
               JOIN folders f ON f.id = p.folder_id
              WHERE p.message_id = ?1 AND p.uid IS NOT NULL
                AND f.role IN ('drafts', 'trash', 'spam')
                AND f.account_id = (SELECT account_id FROM messages WHERE id = ?1)
              ORDER BY f.path, p.uid",
        )?;
        let rows = stmt.query_map(params![draft_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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

    /// Whether this row is a draft this app is writing.
    ///
    /// A second server copy of a received message is still a second message;
    /// only an open draft's own push is compared against what the draft
    /// says, so a copy that merely restates it is not filed beside it.
    pub fn is_composer_draft(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM messages WHERE id = ?1 AND draft_envelope IS NOT NULL",
                params![id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
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

    /// What this store writes on its own server copies of the draft with
    /// this Message-ID, so a later push can tell a copy it wrote from one
    /// another client wrote.
    ///
    /// Every client editing a draft carries its Message-ID, and every Petrel
    /// says `User-Agent: Petrel`, so neither tells this store's copies from
    /// another install's. This is a keyed hash of the Message-ID under a key
    /// made once and kept in `meta`: two stores, or two drafts, never share
    /// one, and one draft's marker says nothing about another's, even after
    /// another client sends that draft on with the header still in it.
    pub fn draft_marker(&self, msgid: &str) -> Result<String> {
        const KEY: &str = "draft_marker_key";
        let kept = self
            .meta(KEY)?
            .and_then(|hex| blake3::Hash::from_hex(hex).ok());
        let key = match kept {
            Some(key) => *key.as_bytes(),
            None => {
                let mut key = [0u8; 32];
                getrandom::fill(&mut key)
                    .map_err(|e| StoreError::Rejected(format!("no random source: {e}")))?;
                self.set_meta(KEY, blake3::Hash::from(key).to_hex().as_str())?;
                key
            }
        };
        let hash = blake3::keyed_hash(&key, msgid.as_bytes());
        Ok(hash.to_hex()[..32].to_string())
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

        // The row, its index entry, its recipients and its folder land
        // together or not at all (docs/25 #92).
        let unit = self.atomic()?;
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

        // A save used to wipe every placement and file the row in Drafts.
        // That is how a draft that had just been trashed walked back into
        // the Drafts list the next time autosave fired, with the composer
        // still open on the same message. The words can still land; the
        // home folder cannot.
        if !self.message_in_role(id, "trash")? && !self.message_in_role(id, "spam")? {
            let folder = self.ensure_folder(account_id, "drafts", "drafts")?;
            self.conn
                .execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
            self.place_message(id, folder)?;
        }
        unit.done()?;
        Ok(id)
    }

    /// Whether this row has a send time: post in the outbox, not a draft.
    pub fn has_send_time(&self, draft_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT send_after_ms IS NOT NULL FROM messages WHERE id = ?1",
                params![draft_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
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
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id FROM messages
             WHERE account_id = ?1 AND {}
             ORDER BY send_after_ms",
            sendable("?2")
        ))?;
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

    /// Takes a message for sending, if it is still due, and says whether it did.
    ///
    /// `due_sends` is read once per pass and the worker sends one message at
    /// a time, so a message pulled back to Drafts, discarded or given a new
    /// time while an earlier one was on the wire was still on the list. It
    /// was claimed anyway, by id alone, and sent: a message the person had
    /// undone went out, and the draft they had just got back was deleted
    /// behind it. This claims only what `due_sends` would still return, in
    /// the same statement.
    pub fn claim_send(&self, id: i64, now_ms: i64) -> Result<bool> {
        let claimed = self.conn.execute(
            &format!(
                "UPDATE messages
                    SET send_state = 'Transmitting', send_error = NULL, send_next_ms = NULL
                  WHERE id = ?1 AND {}",
                sendable("?2")
            ),
            params![id, now_ms],
        )?;
        Ok(claimed == 1)
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
    ///
    /// Only a message still in the outbox, and says whether there was one.
    /// The Outbox redraws once a second, and Send now on a row pulled back a
    /// moment before, by Z or by Edit, queued the draft again while it lay
    /// open in the composer, and it went.
    pub fn resend_now(&self, id: i64, now_ms: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages
                SET send_state = 'RetryQueued', send_error = NULL,
                    send_next_ms = NULL, send_after_ms = ?2
              WHERE id = ?1 AND send_after_ms IS NOT NULL",
            params![id, now_ms],
        )?;
        Ok(n == 1)
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

    /// The draft to open for a message just pulled back out of the outbox:
    /// the message itself, or a new draft with its words if it was deleted
    /// forever while it waited.
    ///
    /// A reply is deleted with its conversation from the Trash and still
    /// goes (see `outbox`). Pulled back, the deleted row was handed over as
    /// the draft. It is in no list, so the draft vanished when the composer
    /// closed, and the grace-period sweep reaped it. Bringing the row back in
    /// place is no better: the actions still queued for its server copy find
    /// a message by its placements and its Message-ID, so they would take
    /// the new draft's copy with them. The deleted row is left to them, and
    /// its words start again as a draft nothing in the queue names.
    pub fn draft_to_reopen(&self, id: i64) -> Result<i64> {
        let deleted: Option<i64> = self
            .conn
            .query_row(
                "SELECT account_id FROM messages WHERE id = ?1 AND deleted_at_ms IS NOT NULL",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(account) = deleted else {
            return Ok(id);
        };
        let d = self.load_draft(id)?;
        self.save_draft_full(
            account,
            None,
            &d.to,
            &d.cc,
            &d.subject,
            &d.body,
            &d.html,
            &d.envelope,
        )
    }

    /// Takes a message out of the outbox to be edited, and returns the draft
    /// to open (`draft_to_reopen`). One transaction: the message never leaves
    /// the queue without a draft to show for it.
    ///
    /// One already pulled back is the same success it always was, with one
    /// exception. When it had been deleted forever, its words are in the
    /// draft the first pull-back made. Opening the deleted row again put the
    /// person's edits where no list shows them and the sweep reaps them, so
    /// that is refused. Z and the Outbox row's Undo can both land within the
    /// row's one-second redraw.
    pub fn pull_back(&self, id: i64) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        let queued = self.queued_state(id)?.is_some();
        let deleted: bool = self
            .conn
            .query_row(
                "SELECT deleted_at_ms IS NOT NULL FROM messages WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if deleted && !queued {
            return Err(StoreError::Rejected(
                "that message is already back in Drafts".into(),
            ));
        }
        self.unschedule_send(id)?;
        let open = if queued {
            self.draft_to_reopen(id)?
        } else {
            id
        };
        tx.commit()?;
        Ok(open)
    }

    /// The send state of a message with a time set, as the outbox would show
    /// it. `None` when it has no time.
    ///
    /// What Edit and Discard ask before they act: one row by id, where
    /// listing the account's whole outbox to find it was the long way round.
    pub fn queued_state(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT coalesce(send_state, 'RetryQueued') FROM messages
                  WHERE id = ?1 AND send_after_ms IS NOT NULL",
                params![id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The outbox, with each row's state spelled out for the UI.
    ///
    /// Every message with a time, wherever it sits. Filing one in the Trash
    /// or Spam, or deleting it forever, does not stop it: a reply went into
    /// the bin with its conversation, and a rule that held binned mail back
    /// held the reply too. Undo, Edit and Discard here are what stop a send,
    /// so everything that will go is listed where they are.
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
        let unit = self.atomic()?;
        // The index row goes with it, or a sent draft's words keep matching.
        self.conn
            .execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        unit.done()
    }

    /// Every file a draft still lists, in any account: saved drafts, and
    /// messages waiting in the outbox, which are drafts with a time.
    ///
    /// For the sweep of staged attachments at launch. A staged file is only
    /// clutter once nothing will send it; one a draft still lists is the
    /// attachment, and sweeping it by age alone failed the send weeks later
    /// with "Could not read attachment". An envelope that does not parse lists
    /// nothing, as it does when the draft is opened.
    pub fn draft_attachment_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT draft_envelope FROM messages WHERE draft_envelope IS NOT NULL")?;
        let envelopes = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(envelopes
            .iter()
            .filter_map(|json| serde_json::from_str::<DraftEnvelope>(json).ok())
            .flat_map(|envelope| envelope.attachments)
            .collect())
    }
}

/// What the worker may send on its own at `now` (a bound parameter): due,
/// and in a state it sends from. `due_sends` and `claim_send` share it, so a
/// claim can never take what the list would not have offered.
///
/// Where the message sits is not asked, on purpose (see `outbox`).
fn sendable(now: &str) -> String {
    format!(
        "send_after_ms IS NOT NULL AND send_after_ms <= {now}
         AND coalesce(send_next_ms, 0) <= {now}
         AND coalesce(send_state, 'RetryQueued') IN ('UndoWindow', 'RetryQueued')"
    )
}
