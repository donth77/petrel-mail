//! Maintenance: the index, the bytes, retention, and the ways out —
//! everything that keeps the store trustworthy over years.
//!
//! Moved verbatim from mod.rs (Phase 1.5).
use super::*;

impl Store {
    pub fn delete_message(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
        tx.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Gives bulk-inserted rows the stored bytes the reading pane renders from.
    ///
    /// `insert_messages` writes headers, a snippet and the index — everything a
    /// list needs and nothing a reader does. A row left that way still claims a
    /// full body, because that is the column default, so it looks perfectly
    /// ordinary in the list and opens blank. Real mail never comes this way:
    /// it arrives through `ingest_raw`, which stores the bytes as it parses
    /// them. This is for synthetic mail, which is seeded in bulk for speed and
    /// still has to be readable.
    ///
    /// One transaction for the batch: a statement pair per message, each
    /// committing on its own, is thousands of fsyncs and turns seeding from
    /// seconds into minutes.
    pub fn attach_raw_bodies(
        &mut self,
        blobs: &crate::blob::BlobStore,
        bodies: &[(i64, Vec<u8>)],
    ) -> Result<usize> {
        let mut written = Vec::with_capacity(bodies.len());
        for (id, raw) in bodies {
            let (hash, size) = blobs
                .write(raw)
                .map_err(|e| StoreError::Ingest(format!("blob write failed: {e}")))?;
            written.push((*id, hash, size, raw.len() as i64));
        }
        let tx = self.conn.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO blobs(hash, kind, size) VALUES (?1, 'raw', ?2)",
            )?;
            let mut upd = tx.prepare_cached(
                "UPDATE messages SET blob_hash = ?2, blob_kind = 'raw', size = ?3 WHERE id = ?1",
            )?;
            for (id, hash, size, raw_len) in &written {
                ins.execute(params![hash, *size as i64])?;
                upd.execute(params![id, hash, raw_len])?;
            }
        }
        tx.commit()?;
        Ok(written.len())
    }

    pub fn update_body(&mut self, id: i64, new_body: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE fts_content SET body_text = ?2 WHERE message_id = ?1",
            params![id, new_body],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Re-derives the indexed text of every stored message from its bytes.
    ///
    /// Runs when the extraction version moves, and not otherwise: it re-parses
    /// every blob, which is cheap for a few hundred messages and not something
    /// to do at every launch. Returns how many were rewritten.
    pub fn reindex_bodies(&mut self, blobs: &crate::blob::BlobStore) -> Result<usize> {
        let held: i64 = self
            .settings()?
            .get("extraction_version")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if held >= Self::EXTRACTION_VERSION {
            return Ok(0);
        }

        let rows: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, blob_hash FROM messages
                 WHERE blob_hash IS NOT NULL AND deleted_at_ms IS NULL",
            )?;
            let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            it.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let tx = self.conn.transaction()?;
        let mut done = 0usize;
        {
            // Both the index and the row preview. The first pass rewrote only
            // the index, which is why placeholders kept showing on rows: the
            // preview is a separate column, written once at ingest, and no
            // amount of reindexing touches it.
            let mut update =
                tx.prepare("UPDATE fts_content SET body_text = ?2 WHERE message_id = ?1")?;
            let mut preview = tx.prepare("UPDATE messages SET snippet = ?2 WHERE id = ?1")?;
            for (id, hash) in rows {
                // A blob that will not read is not a reason to abandon the
                // rest; it keeps whatever text it already had.
                let Ok(raw) = blobs.read(&hash) else { continue };
                let Some(parsed) = petrel_mime::parse_message(&raw) else {
                    continue;
                };
                let text = parsed.index_text();
                update.execute(params![id, &text])?;
                preview.execute(params![id, preview_of(&text)])?;
                done += 1;
            }
        }
        tx.commit()?;
        self.rebuild_fts()?;
        self.set_setting("extraction_version", &Self::EXTRACTION_VERSION.to_string())?;
        Ok(done)
    }

    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('rebuild')",
            [],
        )?;
        // Not external-content, so FTS5's 'rebuild' cannot reach it: repopulate
        // from fts_content, which is the source of truth either way.
        self.conn.execute("DELETE FROM fts_cjk", [])?;
        self.conn.execute(
            "INSERT INTO fts_cjk(rowid, subject, body_text)
             SELECT message_id, petrel_cjk(subject), petrel_cjk(body_text)
             FROM fts_content
             WHERE petrel_has_cjk(subject) OR petrel_has_cjk(body_text)",
            [],
        )?;
        Ok(())
    }

    pub fn optimize_fts(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('optimize')",
            [],
        )?;
        self.conn
            .execute("INSERT INTO fts_cjk(fts_cjk) VALUES('optimize')", [])?;
        Ok(())
    }

    /// Verifies each FTS index against `fts_content`; errors on divergence.
    pub fn fts_integrity_check(&self) -> Result<()> {
        for t in ["fts_messages", "fts_cjk"] {
            let sql = format!("INSERT INTO {t}({t}) VALUES('integrity-check')");
            if let Err(e) = self.conn.execute(&sql, []) {
                return Err(StoreError::Integrity(format!("{t}: {e}")));
            }
        }
        Ok(())
    }

    /// Sets an account's retention mode (Q24).
    pub fn set_local_archive(&self, account_id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET local_archive = ?2 WHERE id = ?1",
            params![account_id, enabled],
        )?;
        Ok(())
    }

    pub fn retention_mode(&self, account_id: i64) -> Result<RetentionMode> {
        let flag: bool = self.conn.query_row(
            "SELECT local_archive FROM accounts WHERE id = ?1",
            params![account_id],
            |r| r.get(0),
        )?;
        Ok(RetentionMode::from_flag(flag))
    }

    /// Applies a server's truth to local state: any message we hold for this
    /// account that the server no longer lists is soft-deleted.
    ///
    /// Soft delete removes it from search and lists immediately (the user asked
    /// for it gone) while keeping the row and blob recoverable until GC. In
    /// `LocalArchive` mode nothing is removed at all — that is the entire point
    /// of the mode, so this returns 0 without touching anything.
    ///
    /// `present_message_ids` are the dedupe keys still on the server.
    pub fn reconcile_server_absences(
        &mut self,
        account_id: i64,
        present_message_ids: &[String],
        now_ms: i64,
    ) -> Result<usize> {
        if self.retention_mode(account_id)? == RetentionMode::LocalArchive {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;
        let present: std::collections::HashSet<&str> =
            present_message_ids.iter().map(|s| s.as_str()).collect();

        let candidates: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, coalesce(message_id_hdr, '') FROM messages
                 WHERE account_id = ?1 AND deleted_at_ms IS NULL",
            )?;
            let rows = stmt.query_map(params![account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut removed = 0;
        for (id, key) in candidates {
            if present.contains(key.as_str()) {
                continue;
            }
            tx.execute(
                "UPDATE messages SET deleted_at_ms = ?2 WHERE id = ?1",
                params![id, now_ms],
            )?;
            // Out of the index immediately: a deleted message must not surface
            // in search while it waits out the grace period.
            tx.execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
            removed += 1;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Restores a soft-deleted message and re-indexes it from its stored bytes.
    /// Possible only while the blob survives — i.e. within the grace period.
    pub fn restore_message(
        &mut self,
        blobs: &crate::blob::BlobStore,
        message_id: i64,
    ) -> Result<bool> {
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT blob_hash FROM messages WHERE id = ?1 AND deleted_at_ms IS NOT NULL",
                params![message_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let Some(hash) = hash else {
            return Ok(false);
        };
        let Ok(raw) = blobs.read(&hash) else {
            return Ok(false);
        };
        let Some(parsed) = petrel_mime::parse_message(&raw) else {
            return Ok(false);
        };

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE messages SET deleted_at_ms = NULL WHERE id = ?1",
            params![message_id],
        )?;
        tx.execute(
            "INSERT INTO fts_content(message_id, subject, body_text, addrs, attachment_names)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(message_id) DO UPDATE SET
                subject = excluded.subject, body_text = excluded.body_text",
            params![
                message_id,
                parsed.subject.clone().unwrap_or_default(),
                parsed.index_text(),
                parsed
                    .addresses()
                    .iter()
                    .map(|(_, a, _)| a.clone())
                    .collect::<Vec<_>>()
                    .join(" "),
                parsed
                    .attachments
                    .iter()
                    .filter_map(|a| a.filename.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Destroys soft-deleted mail whose grace period has expired, then reclaims
    /// any blob no message references any more.
    ///
    /// Blob reclamation is reachability-based rather than per-message, because
    /// blobs are shared by content hash — deleting one message's file could
    /// otherwise blank an identical message in another account.
    /// Plants an action row with no action_messages rows — the shape queue
    /// rows took before that table existed. Tests only: nothing in the
    /// product can create this shape any more, which is the point of the
    /// gc pass that retires it.
    #[doc(hidden)]
    pub fn plant_orphan_action_for_tests(&self, account_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO actions(account_id, kind, payload_json, state, created_ms)
             VALUES (?1, '\"mark_read\"', '{}', 'queued', 1)",
            params![account_id],
        )?;
        Ok(())
    }

    pub fn gc(
        &mut self,
        blobs: &crate::blob::BlobStore,
        now_ms: i64,
        grace_days: i64,
    ) -> Result<GcReport> {
        let cutoff = now_ms.saturating_sub(grace_days.saturating_mul(crate::retention::MS_PER_DAY));

        let tx = self.conn.transaction()?;
        let purged = tx.execute(
            "DELETE FROM messages WHERE deleted_at_ms IS NOT NULL AND deleted_at_ms <= ?1",
            params![cutoff],
        )?;

        // Queued actions with no action_messages rows predate that table and
        // can never be listed by pending_actions, let alone delivered — five
        // of them sat 'queued' for days, invisible to every drain. Named for
        // what they are, so 'queued' keeps meaning "will be tried".
        let orphaned = tx.execute(
            "UPDATE actions SET state = 'orphaned'
             WHERE state = 'queued'
               AND NOT EXISTS (SELECT 1 FROM action_messages am
                                WHERE am.action_id = actions.id)",
            [],
        )?;

        // Orphans: registered blobs no live row points at.
        let orphans: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT b.hash FROM blobs b
                 WHERE NOT EXISTS (SELECT 1 FROM messages m WHERE m.blob_hash = b.hash)
                   AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.blob_hash = b.hash)",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for hash in &orphans {
            tx.execute("DELETE FROM blobs WHERE hash = ?1", params![hash])?;
        }
        tx.commit()?;

        // Files last: a crash here leaves a registered-but-absent blob, which
        // reads as corruption and heals by refetch. The reverse order would
        // delete bytes a live row still points at.
        let mut blobs_removed = 0;
        for hash in &orphans {
            if blobs.remove(hash).is_ok() {
                blobs_removed += 1;
            }
        }

        Ok(GcReport {
            messages_purged: purged,
            blobs_removed,
            actions_orphaned: orphaned,
        })
    }

    /// Ids of the most recent messages — used by maintenance and demo paths that
    /// need to walk the store without materialising whole rows.
    pub fn recent_ids(&self, limit: u32) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM messages WHERE deleted_at_ms IS NULL
             ORDER BY date_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// True when every message came from the synthetic demo generators. Used
    /// to decide whether a store may be wiped for a re-seed; a single real
    /// message makes this false.
    pub fn all_messages_synthetic(&self) -> Result<bool> {
        let foreign: i64 = self.conn.query_row(
            "SELECT count(*) FROM messages
             WHERE from_addr NOT LIKE '%example.com'
               AND from_addr NOT LIKE '%example.org'
               AND from_addr NOT LIKE '%example.net'
               AND from_addr NOT LIKE '%example.io'
               AND from_addr NOT LIKE '%example.dev'
               AND from_addr NOT LIKE '%.example'
               AND from_addr NOT LIKE '%example.jp'",
            [],
            |r| r.get(0),
        )?;
        Ok(foreign == 0)
    }

    /// Demo-path only: empties the mailbox. Callers must have established that
    /// the store holds nothing real.
    pub fn delete_all_messages(&self) -> Result<usize> {
        let n = self.conn.execute("DELETE FROM messages", [])?;
        self.conn.execute("DELETE FROM fts_content", [])?;
        self.conn.execute("DELETE FROM fts_cjk", [])?;
        self.conn.execute(
            "INSERT INTO fts_messages(fts_messages) VALUES('rebuild')",
            [],
        )?;
        Ok(n)
    }

    /// Writes a view's messages to an mbox file, oldest first.
    ///
    /// mbox because it is the format every other mail client can read — the
    /// promise being kept here is that your mail is yours whatever happens to
    /// this program, and a proprietary export would break that promise while
    /// appearing to honour it.
    ///
    /// Each message is written with two headers of Petrel's own —
    /// `X-Petrel-Folders` and `X-Petrel-Tags` — naming where it sat and what it
    /// wore. mbox is one flat stream with no idea of a hierarchy, so without
    /// them an archive of a mailbox that files its history under `Archive/2023`
    /// arrives as an undifferentiated heap and the filing is simply gone. Gmail
    /// Takeout solves it the same way with `X-Gmail-Labels`, which makes this
    /// the closest thing to a convention that exists. Unknown `X-` headers are
    /// ignored by every reader, so the file stays ordinary mbox — and they sit
    /// outside any DKIM `h=` list, so no signature is disturbed.
    ///
    /// Returns how many messages were written. Messages whose blob is missing
    /// are skipped and counted separately rather than aborting the export: a
    /// partial archive of 9,000 messages is worth more than an error and none.
    pub fn export_mbox(
        &self,
        blobs: &crate::blob::BlobStore,
        account: i64,
        view: &ListView,
        path: &Path,
    ) -> Result<(usize, usize)> {
        use std::io::Write;

        let ids: Vec<(i64, String, String, i64)> = {
            // Every message in the view's conversations, not just the newest of
            // each: an archive that keeps one message per thread is not an
            // archive of your mail.
            //
            // The account is named rather than taken from whichever is
            // active: an export is addressed to a person by their mailbox,
            // and "the one on screen" is not something the file can record.
            let threads: Vec<i64> = self
                .list_threads_for(account, view, 0, u32::MAX, Sort::default())?
                .into_iter()
                .map(|t| t.thread_id)
                .collect();
            let mut out = Vec::new();
            for tid in threads {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT id, coalesce(blob_hash,''), coalesce(from_addr,''), date_ms
                     FROM messages
                     WHERE coalesce(thread_id, -id) = ?1 AND deleted_at_ms IS NULL
                     ORDER BY date_ms",
                )?;
                let rows = stmt.query_map(params![tid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
                for row in rows {
                    out.push(row?);
                }
            }
            out.sort_by_key(|(_, _, _, date)| *date);
            out
        };

        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut written = 0usize;
        let mut skipped = 0usize;

        let mut folders_of = self.conn.prepare_cached(
            "SELECT f.path FROM placements p
             JOIN folders f ON f.id = p.folder_id
             WHERE p.message_id = ?1 ORDER BY f.path",
        )?;
        let mut tags_of = self.conn.prepare_cached(
            "SELECT t.name FROM message_tags mt
             JOIN tags t ON t.id = mt.tag_id
             WHERE mt.message_id = ?1 ORDER BY t.name COLLATE NOCASE",
        )?;
        let strings = |stmt: &mut rusqlite::CachedStatement<'_>, id: i64| -> Result<Vec<String>> {
            let rows = stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        };

        for (id, hash, from, date_ms) in ids {
            let Ok(raw) = blobs.read(&hash) else {
                skipped += 1;
                continue;
            };
            // The "From " line mbox separates on, in the asctime shape readers
            // expect. Anything unparseable still gets a line, because a reader
            // that cannot find the separator sees one enormous message.
            let stamp = format_asctime(date_ms);
            let sender = if from.is_empty() {
                "petrel@localhost"
            } else {
                &from
            };
            writeln!(file, "From {sender} {stamp}")?;

            // Ahead of the message's own headers, which is where a reader
            // expects to find headers and where Takeout puts its own.
            if let Some(line) = metadata_header("X-Petrel-Folders", &strings(&mut folders_of, id)?)
            {
                writeln!(file, "{line}")?;
            }
            if let Some(line) = metadata_header("X-Petrel-Tags", &strings(&mut tags_of, id)?) {
                writeln!(file, "{line}")?;
            }

            // ">From " escaping: a body line beginning "From " would otherwise
            // start a new message when the file is read back, silently splitting
            // one message into two.
            for line in raw.split(|b| *b == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.starts_with(b"From ") {
                    file.write_all(b">")?;
                }
                file.write_all(line)?;
                file.write_all(b"\n")?;
            }
            file.write_all(b"\n")?;
            written += 1;
        }
        file.flush()?;
        Ok((written, skipped))
    }

    /// Counts and bytes for the Storage pane.
    ///
    /// The index size comes from the FTS tables rather than being inferred from
    /// the file: the point of showing it separately is that it is the part you
    /// can rebuild, so a number that silently includes the mail is useless.
    ///
    /// Mail bytes come from the `blobs` ledger, not from walking the blob
    /// directory. The ledger records each blob's on-disk (compressed) size as
    /// it is written, so the two agree to the byte — and the walk was a
    /// `stat()` per message, most of a second on a mailbox of any size, which
    /// is a long time for a settings pane to sit blank.
    /// Messages that exist on this computer and nowhere else.
    ///
    /// The number a person needs before they wipe the store. Everything synced
    /// from a server can be fetched again; mail imported from an mbox, and
    /// anything filed into a local folder, cannot. Deleting the store takes
    /// those with it and no resync brings them back.
    ///
    /// A message counts when it has placements and every one of them is in a
    /// local folder. One copy still on a server is enough to make it
    /// recoverable, which is why this asks "all" rather than "any" — erring
    /// the other way would inflate the warning and teach people to dismiss it.
    pub fn local_only_messages(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM (
               SELECT p.message_id
                 FROM placements p
                 JOIN folders f ON f.id = p.folder_id
                 JOIN messages m ON m.id = p.message_id
                WHERE m.deleted_at_ms IS NULL
                GROUP BY p.message_id
               HAVING sum(
                        CASE WHEN coalesce(
                          json_extract(f.sync_state_json, '$.local'), 0) IN (1, 'true')
                        THEN 0 ELSE 1 END
                      ) = 0
             )",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn storage_report(&self, db_path: &Path) -> Result<StorageReport> {
        let file = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let database_bytes = file(db_path.to_path_buf())
            + file(db_path.with_extension("db-wal"))
            + file(db_path.with_extension("db-shm"));

        let blob_bytes: u64 = self
            .conn
            .query_row("SELECT coalesce(sum(size), 0) FROM blobs", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n.max(0) as u64)?;

        // dbstat is a compile-time option; when it is missing the honest answer
        // is zero rather than a guess that would be wrong by an order of
        // magnitude either way.
        let index_bytes: u64 = self
            .conn
            .query_row(
                "SELECT coalesce(sum(pgsize), 0) FROM dbstat
                 WHERE name LIKE 'fts_%'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u64)
            .unwrap_or(0);

        // Per account. The bytes are deduplicated within an account (the hash
        // set is a set) but not across accounts — see `AccountStorage`.
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.id,
                    (SELECT count(*) FROM messages m WHERE m.account_id = a.id),
                    (SELECT coalesce(sum(b.size), 0) FROM blobs b WHERE b.hash IN (
                         SELECT blob_hash FROM messages
                          WHERE account_id = a.id AND blob_hash IS NOT NULL
                         UNION
                         SELECT at.blob_hash FROM attachments at
                           JOIN messages m ON m.id = at.message_id
                          WHERE m.account_id = a.id AND at.blob_hash IS NOT NULL))
               FROM accounts a ORDER BY a.id",
        )?;
        let accounts = stmt
            .query_map([], |r| {
                Ok(AccountStorage {
                    account_id: r.get(0)?,
                    messages: r.get(1)?,
                    blob_bytes: r.get::<_, i64>(2)?.max(0) as u64,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(StorageReport {
            messages: self.message_count()?,
            attachments: self
                .conn
                .query_row("SELECT count(*) FROM attachments", [], |r| r.get(0))?,
            database_bytes,
            blob_bytes,
            index_bytes,
            accounts,
        })
    }

    /// Messages held for one account — what the account's own views can
    /// honestly report, where the global count would smuggle in every other
    /// account's mail.
    pub fn message_count_for(&self, account_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM messages WHERE account_id = ?1 AND deleted_at_ms IS NULL",
            params![account_id],
            |r| r.get(0),
        )?)
    }

    pub fn message_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?)
    }

    pub fn set_has_attachments(&self, message_id: i64, yes: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET has_attachments = ?2 WHERE id = ?1",
            params![message_id, yes as i64],
        )?;
        Ok(())
    }

    /// Whether this message's remote content may load.
    ///
    /// Blocked unless one of two things is true:
    ///
    /// * the sender has been trusted explicitly, or
    /// * the user has written to them.
    ///
    /// The second is the one that makes blocking liveable. A tracking pixel
    /// buys the sender three facts — the address is real, when it was read, and
    /// roughly from where. Someone the user has already emailed has the first
    /// two by other means and is not a stranger, so the trade is no longer
    /// worth breaking their mail over. It is deliberately a question about sent
    /// mail rather than a stored flag, so it answers correctly for a
    /// correspondent from before this feature existed, and stops being true if
    /// that mail is ever removed.
    pub fn remote_content_allowed(&self, message_id: i64) -> Result<bool> {
        let row: Option<(i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT account_id, from_addr FROM messages WHERE id = ?1",
                params![message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        // No sender to judge means no reason to trust it.
        let Some((account_id, Some(from))) = row else {
            return Ok(false);
        };
        let from = from.trim().to_lowercase();
        if from.is_empty() {
            return Ok(false);
        }
        Ok(self.sender_trusted(account_id, &from)? || self.has_written_to(account_id, &from)?)
    }

    /// Addresses to offer while a recipient is being typed, best first.
    ///
    /// Harvested from mail already synced — there is no lookup anywhere else,
    /// which is the point: a composer that phones a server to ask who you might
    /// be writing to has told it who you are writing to.
    ///
    /// Ranked by frecency rather than either half alone. Frequency by itself
    /// buries the person you emailed twice this morning under a mailing list
    /// from three years ago; recency by itself puts whoever mailed you last
    /// above your closest colleague. The weights below are deliberately blunt:
    ///
    /// * **Written to, hugely.** Somebody you have sent mail to is a different
    ///   kind of thing from an address that has appeared in your inbox, and no
    ///   amount of newsletter volume should outrank one.
    /// * **Seen often**, linearly, capped so a mailing list cannot dominate.
    /// * **Seen lately**, in two coarse steps rather than a curve — the
    ///   difference between last week and last month matters, the difference
    ///   between fourteen and fifteen months does not.
    ///
    /// Spam is excluded, and so is the user's own address: a completion list
    /// that offers to send mail to yourself is answering a question nobody
    /// asked.
    pub fn complete_addresses(
        &self,
        account_id: i64,
        prefix: &str,
        now_ms: i64,
        limit: u32,
    ) -> Result<Vec<Correspondent>> {
        let needle = prefix.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        // Matched at the start of the address, of its domain, or of any word in
        // the display name — the three places people begin typing. Not a bare
        // substring: "an" should not offer everyone with "an" mid-surname.
        let starts = format!("{needle}%");
        let word = format!("% {needle}%");
        let at = format!("%@{needle}%");
        let month = now_ms - 30 * crate::retention::MS_PER_DAY;
        let quarter = now_ms - 90 * crate::retention::MS_PER_DAY;

        let mut stmt = self.conn.prepare_cached(
            "SELECT ma.addr_norm,
                    coalesce(max(nullif(ma.display, '')), '') AS display,
                    max(CASE WHEN EXISTS (
                          SELECT 1 FROM placements p
                          JOIN folders f ON f.id = p.folder_id
                          WHERE p.message_id = m.id AND f.role = 'sent')
                        AND ma.role IN ('to', 'cc')
                        THEN 1 ELSE 0 END) AS written,
                    count(*) AS seen,
                    max(m.date_ms) AS last_ms
             FROM message_addresses ma
             JOIN messages m ON m.id = ma.message_id
             WHERE m.account_id = ?1
               AND m.deleted_at_ms IS NULL
               AND ma.addr_norm <> ''
               AND NOT EXISTS (SELECT 1 FROM placements p
                               JOIN folders f ON f.id = p.folder_id
                               WHERE p.message_id = m.id AND f.role = 'spam')
               AND ma.addr_norm <> coalesce(
                     (SELECT lower(email) FROM accounts WHERE id = ?1), '')
               AND (ma.addr_norm LIKE ?2
                    OR lower(ma.display) LIKE ?2
                    OR lower(ma.display) LIKE ?3
                    OR ma.addr_norm LIKE ?4)
             GROUP BY ma.addr_norm
             ORDER BY written DESC,
                      min(seen, 20)
                        + CASE WHEN last_ms > ?5 THEN 25
                               WHEN last_ms > ?6 THEN 10
                               ELSE 0 END DESC,
                      last_ms DESC
             LIMIT ?7",
        )?;
        let rows = stmt.query_map(
            params![account_id, starts, word, at, month, quarter, limit],
            |r| {
                Ok(Correspondent {
                    addr: r.get(0)?,
                    display: r.get(1)?,
                    written_to: r.get::<_, i64>(2)? != 0,
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Who sent a message, normalised the way the trust list stores addresses.
    pub fn message_sender(&self, message_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT from_addr FROM messages WHERE id = ?1",
                params![message_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|a| a.trim().to_lowercase())
            .filter(|a| !a.is_empty()))
    }

    /// Whether this address was trusted by hand.
    pub fn sender_trusted(&self, account_id: i64, addr: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM remote_senders
                            WHERE account_id = ?1 AND addr_norm = ?2)",
            params![account_id, addr.trim().to_lowercase()],
            |r| r.get::<_, i64>(0),
        )? != 0)
    }

    /// Whether the user has ever sent mail to this address.
    ///
    /// Sent mail only — being written *to* by someone says nothing about
    /// whether they know you, which is the entire question.
    pub fn has_written_to(&self, account_id: i64, addr: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM message_addresses ma
               JOIN messages m ON m.id = ma.message_id
               JOIN placements p ON p.message_id = m.id
               JOIN folders f ON f.id = p.folder_id
               WHERE m.account_id = ?1
                 AND m.deleted_at_ms IS NULL
                 AND f.role = 'sent'
                 AND ma.role IN ('to', 'cc')
                 AND ma.addr_norm = ?2)",
            params![account_id, addr.trim().to_lowercase()],
            |r| r.get::<_, i64>(0),
        )? != 0)
    }

    /// Trusts a sender's remote content from now on.
    pub fn trust_sender(&self, account_id: i64, addr: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_senders(account_id, addr_norm, added_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, addr_norm) DO UPDATE SET added_ms = excluded.added_ms",
            params![account_id, addr.trim().to_lowercase(), now_ms],
        )?;
        Ok(())
    }

    /// Takes that trust back.
    pub fn untrust_sender(&self, account_id: i64, addr: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM remote_senders WHERE account_id = ?1 AND addr_norm = ?2",
            params![account_id, addr.trim().to_lowercase()],
        )?;
        Ok(())
    }

    /// The trusted senders, most recently trusted first — what the privacy
    /// pane lists so a decision made once in a banner can be found and undone.
    pub fn trusted_senders(&self, account_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT addr_norm FROM remote_senders WHERE account_id = ?1
             ORDER BY added_ms DESC, addr_norm",
        )?;
        let rows = stmt.query_map(params![account_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn db_size_bytes(&self) -> Result<i64> {
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(pages * page_size)
    }
}

/// One `X-Petrel-*` header line, or nothing when there is nothing to say.
///
/// Three jobs, all of them about producing a file that is still valid mail.
///
/// A value cannot carry a line ending. A folder path is whatever the server
/// named it and a tag is whatever the user typed, so both are untrusted on
/// their way into a header — a newline in either would end the header block
/// early and turn the rest of the name into a forged header, or into body.
///
/// A value containing a comma or a quote is quoted, since the list separator
/// is a comma and `Archive/Invoices, paid` would otherwise read as two names.
/// This is what `X-Gmail-Labels` does, and matching it means an importer
/// written for Takeout reads these too.
///
/// And the line is folded at 78 columns onto continuation lines, because RFC
/// 5322 allows 998 and recommends 78, and a mailbox with forty labels on one
/// message will pass both without folding.
fn metadata_header(name: &str, values: &[String]) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let mut line = format!("{name}:");
    let mut column = line.len();
    for (i, raw) in values.iter().enumerate() {
        let clean: String = raw
            .chars()
            .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
            .collect();
        let value = if clean.contains(',') || clean.contains('"') {
            format!("\"{}\"", clean.replace('"', "\\\""))
        } else {
            clean
        };
        let token = if i + 1 < values.len() {
            format!("{value},")
        } else {
            value
        };
        // Fold before a token that would overrun, never after — a trailing
        // space at the end of a header line is significant to some parsers.
        if column + 1 + token.chars().count() > 78 && column > name.len() + 1 {
            line.push_str("\n ");
            column = 1;
        } else {
            line.push(' ');
            column += 1;
        }
        column += token.chars().count();
        line.push_str(&token);
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::metadata_header;

    #[test]
    fn nothing_to_say_writes_no_header() {
        assert_eq!(metadata_header("X-Petrel-Tags", &[]), None);
    }

    #[test]
    fn names_are_listed_comma_separated() {
        let h = metadata_header(
            "X-Petrel-Folders",
            &["Archive/2023".into(), "Receipts".into()],
        );
        assert_eq!(
            h.as_deref(),
            Some("X-Petrel-Folders: Archive/2023, Receipts")
        );
    }

    #[test]
    fn a_name_holding_the_separator_is_quoted() {
        let h = metadata_header("X-Petrel-Tags", &["urgent, maybe".into()]);
        assert_eq!(h.as_deref(), Some("X-Petrel-Tags: \"urgent, maybe\""));
        // And a quote inside one is escaped rather than ending the quoting.
        let h = metadata_header("X-Petrel-Tags", &["say \"hi\", now".into()]);
        assert_eq!(h.as_deref(), Some("X-Petrel-Tags: \"say \\\"hi\\\", now\""));
    }

    /// The one that matters for safety: a header cannot be made to end early.
    #[test]
    fn a_line_ending_in_a_name_cannot_forge_a_header() {
        let h = metadata_header(
            "X-Petrel-Tags",
            &["evil\r\nBcc: someone@example.com".into()],
        );
        let h = h.unwrap();
        assert!(!h.contains('\r'), "{h}");
        assert!(!h.contains("\nBcc:"), "{h}");
        assert!(h.contains("Bcc: someone@example.com"), "{h}");
    }

    #[test]
    fn a_long_list_folds_onto_continuation_lines() {
        let many: Vec<String> = (0..12)
            .map(|i| format!("Archive/Year/{i}0000000"))
            .collect();
        let h = metadata_header("X-Petrel-Folders", &many).unwrap();
        assert!(h.contains("\n "), "never folded: {h}");
        for line in h.split('\n') {
            assert!(line.chars().count() <= 78, "line too long: {line:?}");
        }
        // Folding is whitespace, so unfolding recovers the list exactly.
        let unfolded = h.replace("\n ", " ");
        for one in &many {
            assert!(unfolded.contains(one.as_str()), "lost {one} in {unfolded}");
        }
    }
}
