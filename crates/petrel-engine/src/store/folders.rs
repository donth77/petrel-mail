//! Folders: places on the server, mirrored here.
//!
//! Moved verbatim from mod.rs (Phase 1.5 of the close-out plan) — a child
//! module sees the parent's private items, so `Store`'s fields and the
//! helpers stay where they were and nothing changed visibility to make
//! this split possible. Behavior lives in the tests, which did not move.
use super::*;

impl Store {
    /// The folder holding a role for this account, if one is mapped.
    pub fn folder_for_role(&self, account_id: i64, role: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND role = ?2 LIMIT 1",
                params![account_id, role],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Creates a folder for a role if the account has none. Real accounts learn
    /// their folders from the server; this exists for accounts that have not
    /// synced and for tests, so triage has somewhere to move mail to.
    pub fn ensure_folder(&self, account_id: i64, role: &str, path: &str) -> Result<i64> {
        if let Some(id) = self.folder_for_role(account_id, role)? {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO folders(account_id, role, name, path) VALUES (?1, ?2, ?3, ?4)",
            params![account_id, role, path, path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Every folder on the account, for the move picker.
    ///
    /// Folders the user made come first, because those are what a move is
    /// usually for; the role folders are reachable from the rail already and
    /// are here only so V can get to them too.
    pub fn folders(&self, account_id: i64) -> Result<Vec<FolderSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(role,''), path FROM folders
             WHERE account_id = ?1
             ORDER BY (role IS NOT NULL AND role <> ''), path COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok(FolderSummary {
                id: r.get(0)?,
                role: r.get(1)?,
                path: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Records the folders a server reported, with the roles it claimed.
    ///
    /// Upsert by path rather than insert: a resync must not duplicate every
    /// folder, and a server that starts advertising SPECIAL-USE later should
    /// update the role on the row that already exists rather than shadow it.
    pub fn sync_folders(
        &mut self,
        account_id: i64,
        folders: &[(String, Option<String>)],
    ) -> Result<usize> {
        let mut n = 0;
        for (path, role) in folders {
            let name = path.rsplit('/').next().unwrap_or(path);
            let existing: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM folders WHERE account_id = ?1 AND path = ?2",
                    params![account_id, path],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(id) => {
                    self.conn.execute(
                        "UPDATE folders SET role = ?2, name = ?3 WHERE id = ?1",
                        params![id, role, name],
                    )?;
                }
                None => {
                    self.conn.execute(
                        "INSERT INTO folders(account_id, role, name, path) VALUES (?1, ?2, ?3, ?4)",
                        params![account_id, role, name, path],
                    )?;
                }
            }
            n += 1;
        }
        // Folders the server no longer lists are gone — renamed elsewhere,
        // deleted elsewhere, or (the day this was written) a \Noselect
        // container that stopped being reported as a mailbox. Their rows and
        // placements go; their mail stays, as it does everywhere else here.
        let known: std::collections::HashSet<&str> =
            folders.iter().map(|(p, _)| p.as_str()).collect();
        let stale: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, path FROM folders WHERE account_id = ?1")?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.filter_map(|r| r.ok())
                .filter(|(_, path)| !known.contains(path.as_str()))
                .map(|(id, _)| id)
                // A local folder is absent from every survey by definition —
                // imported mail lives in one, and pruning it would delete the
                // only placements that mail has.
                .filter(|id| !self.folder_is_local(*id).unwrap_or(false))
                .collect()
        };
        for id in stale {
            self.remove_folder(id)?;
        }
        Ok(n)
    }

    /// A folder the user named, looked up by path and created if new.
    ///
    /// Separate from `ensure_folder`, which keys on role: two user folders can
    /// exist side by side with no role at all, so role is the wrong identity
    /// for them.
    pub fn ensure_named_folder(&self, account_id: i64, path: &str) -> Result<i64> {
        let path = path.trim();
        if path.is_empty() {
            return Err(StoreError::Rejected("a folder needs a name".into()));
        }
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND path = ?2 COLLATE NOCASE",
                params![account_id, path],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        // The leaf is the display name; the path keeps the hierarchy the server
        // uses, so "Contracts/2026" shows as 2026 nested under Contracts.
        let name = path.rsplit('/').next().unwrap_or(path);
        self.conn.execute(
            "INSERT INTO folders(account_id, role, name, path) VALUES (?1, NULL, ?2, ?3)",
            params![account_id, name, path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Renames a folder — the local half, after the server has agreed.
    ///
    /// The row id is the identity, so placements, counts and the open view
    /// all survive the rename untouched; only the words change.
    pub fn rename_folder(&mut self, folder_id: i64, new_path: &str) -> Result<()> {
        let (account, old_path): (i64, String) = self.conn.query_row(
            "SELECT account_id, path FROM folders WHERE id = ?1",
            params![folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let name = new_path.rsplit(['/', '.']).next().unwrap_or(new_path);
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE folders SET path = ?2, name = ?3 WHERE id = ?1",
            params![folder_id, new_path, name],
        )?;
        // The server renames a subtree in one RENAME (RFC 3501); the local
        // tree follows suit, or every descendant's path goes quietly stale
        // and the next survey prunes real folders as strangers.
        for delim in ['/', '.'] {
            tx.execute(
                "UPDATE folders
                 SET path = ?3 || substr(path, length(?2) + 1)
                 WHERE account_id = ?1 AND path LIKE ?2 || ?4",
                params![account, old_path, new_path, format!("{delim}%")],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets a folder — the local half, after the server has deleted it.
    ///
    /// Placements go because the location is gone; message rows and blobs
    /// stay, exactly as UIDVALIDITY recovery keeps them: removing a folder is
    /// not a licence to destroy the mail that passed through it. A message
    /// left with no placement drops out of folder views and remains findable
    /// in search.
    pub fn remove_folder(&mut self, folder_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM placements WHERE folder_id = ?1",
            params![folder_id],
        )?;
        tx.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn place_message(&self, message_id: i64, folder_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO placements(message_id, folder_id) VALUES (?1, ?2)",
            params![message_id, folder_id],
        )?;
        Ok(())
    }

    pub fn folders_of(&self, message_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT folder_id FROM placements WHERE message_id = ?1")?;
        let rows = stmt.query_map(params![message_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// How this account's provider models placement.
    ///
    /// Derived from the account kind rather than sniffed per action, so every
    /// caller gets the same answer and a mis-detected provider is one row to
    /// fix rather than a behaviour scattered across call sites. Unknown
    /// providers get the exclusive model: it is the IMAP default, and guessing
    /// "labels" for a server that does not have them would leave messages in
    /// two folders that can only hold them in one.
    pub fn placement_policy(&self, account_id: i64) -> Result<crate::actions::PlacementPolicy> {
        use crate::actions::PlacementPolicy;
        let kind: String = self
            .conn
            .query_row(
                "SELECT kind FROM accounts WHERE id = ?1",
                params![account_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        Ok(match kind.as_str() {
            "gmail" => PlacementPolicy::Labels,
            _ => PlacementPolicy::Exclusive,
        })
    }

    /// The highest UID stored for a folder, or None when it holds nothing yet.
    ///
    /// This is what makes a poll cheap: everything above it is new, so a resync
    /// asks for `{max+1}:*` rather than refetching the last N messages by
    /// sequence number every time — which is both wasteful and wrong, since
    /// sequence numbers shift as mail arrives and is expunged.
    pub fn max_uid(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT max(uid) FROM placements WHERE folder_id = ?1",
                params![folder_id],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .map(|u| u as u32))
    }

    /// The UIDVALIDITY the folder's stored UIDs were recorded under.
    ///
    /// `None` means it was never recorded — the folder has not completed a
    /// sync since this began being tracked — which callers treat as "adopt
    /// whatever the server says", never as a mismatch.
    pub fn folder_validity(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT uidvalidity FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten()
            .map(|v| v as u32))
    }

    /// Records the UIDVALIDITY the folder's UIDs now belong to.
    ///
    /// In a recovery this is deliberately the *last* step, after the re-map
    /// and the re-fetch of unknowns: a crash anywhere earlier leaves the old
    /// value in place, so the next pass sees the mismatch again and re-runs
    /// recovery — which is idempotent — instead of trusting half-mended UIDs.
    pub fn set_folder_validity(&mut self, folder_id: i64, v: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET uidvalidity = ?2 WHERE id = ?1",
            params![folder_id, v.map(|v| v as i64)],
        )?;
        Ok(())
    }

    /// Mends a folder after the server renumbered it (UIDVALIDITY reset).
    ///
    /// The folder is still the folder and the mail is still the mail; what
    /// died is the numbering. Every stored UID is first quarantined to NULL —
    /// a NULL UID is already how "not addressable on the server" is spelled
    /// here, so queued actions hold automatically instead of firing at
    /// whatever message inherited the number. Then each server (uid,
    /// Message-ID) pair is matched against what the store already holds, by
    /// the same key ingest dedupes on, and the placement learns its new UID.
    ///
    /// What the server listed but the store cannot match comes back in
    /// `to_fetch`: those are downloaded again in full. Worst case a reset
    /// costs re-download — never data, which is why nothing here touches a
    /// message row or a blob.
    ///
    /// Placements still NULL after the pass are dropped only when `complete`
    /// says the listing covered the whole mailbox. A depth-limited listing
    /// proves nothing about mail older than its window, and evicting history
    /// because the window ended is exactly the data loss this exists to avoid.
    pub fn remap_folder_after_reset(
        &mut self,
        folder_id: i64,
        server: &[(u32, Option<String>)],
        complete: bool,
    ) -> Result<RemapOutcome> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE placements SET uid = NULL WHERE folder_id = ?1",
            params![folder_id],
        )?;
        let account: i64 = tx.query_row(
            "SELECT account_id FROM folders WHERE id = ?1",
            params![folder_id],
            |r| r.get(0),
        )?;
        let mut out = RemapOutcome::default();
        for (uid, mid) in server {
            let matched = match mid {
                Some(mid) => tx.execute(
                    "UPDATE placements SET uid = ?1 WHERE folder_id = ?2 AND message_id = (
                         SELECT id FROM messages WHERE account_id = ?3 AND message_id_hdr = ?4
                     )",
                    params![*uid as i64, folder_id, account, mid],
                )?,
                // No Message-ID on the wire: indistinguishable from mail we
                // have (whose ingest key was a blob hash), so refetch it and
                // let ingest's own dedupe decide.
                None => 0,
            };
            if matched > 0 {
                out.rematched += 1;
            } else {
                out.to_fetch.push(*uid);
            }
        }
        if complete {
            out.dropped = tx.execute(
                "DELETE FROM placements WHERE folder_id = ?1 AND uid IS NULL",
                params![folder_id],
            )?;
        }
        tx.commit()?;
        Ok(out)
    }

    /// The message this account holds under a wire Message-ID, if any.
    /// Writes a UID the server has just confirmed onto the one placement that
    /// lost its number — and only onto one that lost it. A placement already
    /// holding a UID belongs to the sync; the drain never overrules it.
    pub fn heal_placement_uid(
        &self,
        message_id: i64,
        account_id: i64,
        folder_path: &str,
        uid: u32,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE placements SET uid = ?1
             WHERE message_id = ?2 AND uid IS NULL
               AND folder_id = (SELECT id FROM folders
                                 WHERE account_id = ?3 AND path = ?4)",
            params![uid as i64, message_id, account_id, folder_path],
        )?;
        Ok(n > 0)
    }

    /// How many placements in this folder claim a server UID — the number a
    /// folder's STATUS should agree with, ghosts aside.
    pub fn uid_placement_count(&self, folder_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM placements WHERE folder_id = ?1 AND uid IS NOT NULL",
            params![folder_id],
            |r| r.get(0),
        )?)
    }

    /// Drops the placements whose UIDs the server no longer answers for.
    ///
    /// The windowed sync only ever adds: a message moved out on the server —
    /// by our own drain, another client, or a rule — left its old placement
    /// behind forever, and a conversation haunted both its folder and the
    /// inbox. Given the folder's actual UID set, everything stored but absent
    /// goes. NULL-UID placements stay: they are local or quarantined, and the
    /// server not naming them is exactly what is already known about them.
    pub fn remove_placements_absent(
        &self,
        folder_id: i64,
        present: &std::collections::HashSet<u32>,
    ) -> Result<usize> {
        let stored: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT uid FROM placements WHERE folder_id = ?1 AND uid IS NOT NULL")?;
            let rows = stmt.query_map(params![folder_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let gone: Vec<i64> = stored
            .into_iter()
            .filter(|u| !present.contains(&(*u as u32)))
            .collect();
        for chunk in gone.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&folder_id];
            for u in chunk {
                params.push(u);
            }
            self.conn.execute(
                &format!("DELETE FROM placements WHERE folder_id = ?1 AND uid IN ({marks})"),
                rusqlite::params_from_iter(params),
            )?;
        }
        Ok(gone.len())
    }

    /// Every UID this folder's placements carry — the store's side of the
    /// reconciliation diff.
    pub fn placement_uids(&self, folder_id: i64) -> Result<Vec<u32>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uid FROM placements WHERE folder_id = ?1 AND uid IS NOT NULL")?;
        let rows = stmt.query_map(params![folder_id], |r| r.get::<_, i64>(0))?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|u| u as u32)
            .collect())
    }

    /// The UID a message's placement in one folder carries, if it is placed
    /// there at all. `None`: not placed. `Some(None)`: placed without a
    /// number — local or quarantined.
    pub fn placement_uid(&self, message_id: i64, folder_id: i64) -> Result<Option<Option<i64>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT uid FROM placements WHERE message_id = ?1 AND folder_id = ?2",
                params![message_id, folder_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?)
    }

    /// Removes one message's placement in one folder — what a delivered move
    /// means locally. The drain calls this the moment the server confirms,
    /// so a fetch that raced the delivery cannot leave the old placement
    /// haunting the folder it came from.
    pub fn remove_placement(
        &self,
        message_id: i64,
        account_id: i64,
        folder_path: &str,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM placements
             WHERE message_id = ?1
               AND folder_id = (SELECT id FROM folders
                                 WHERE account_id = ?2 AND path = ?3)",
            params![message_id, account_id, folder_path],
        )?;
        Ok(n > 0)
    }

    pub fn message_by_msgid(&self, account_id: i64, msgid: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM messages
                 WHERE account_id = ?1 AND message_id_hdr = ?2 AND deleted_at_ms IS NULL",
                params![account_id, msgid],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Records that a folder holds a message at a UID, without touching the
    /// message itself — how the All Mail walk claims mail it already has.
    pub fn place_message_at(&self, message_id: i64, folder_id: i64, uid: u32) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO placements(message_id, folder_id, uid) VALUES (?1, ?2, ?3)",
            params![message_id, folder_id, uid as i64],
        )?;
        Ok(())
    }

    /// Marks a folder as local-only: it exists in this store and nowhere
    /// else. The sync survey never prunes it (the server not listing it is
    /// the point) and the sync loop never asks the server about it.
    pub fn mark_folder_local(&mut self, folder_id: i64) -> Result<()> {
        let current: String = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "{}".into());
        let mut v: serde_json::Value =
            serde_json::from_str(&current).unwrap_or_else(|_| serde_json::json!({}));
        v["local"] = serde_json::json!(true);
        self.conn.execute(
            "UPDATE folders SET sync_state_json = ?2 WHERE id = ?1",
            params![folder_id, v.to_string()],
        )?;
        Ok(())
    }

    pub fn folder_is_local(&self, folder_id: i64) -> Result<bool> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.get("local").and_then(|m| m.as_bool()))
            .unwrap_or(false))
    }

    /// The lowest UID this folder holds — the backfill cursor, resumable by
    /// construction: whatever is below it has not been fetched yet.
    pub fn min_uid(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT min(uid) FROM placements WHERE folder_id = ?1",
                params![folder_id],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .map(|u| u as u32))
    }

    /// The lowest UID backfill has *asked for* in this folder. Distinct from
    /// `min_uid`, which is the lowest we hold: a range whose messages were
    /// long since expunged fetches nothing, and without this floor the walk
    /// would re-ask for the same silence forever. Floor 1 means done.
    pub fn backfill_floor(&self, folder_id: i64) -> Result<Option<u32>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.get("backfill_floor").and_then(|m| m.as_u64()))
            .map(|v| v as u32))
    }

    pub fn set_backfill_floor(&mut self, folder_id: i64, floor: u32) -> Result<()> {
        let current: String = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "{}".into());
        let mut v: serde_json::Value =
            serde_json::from_str(&current).unwrap_or_else(|_| serde_json::json!({}));
        v["backfill_floor"] = serde_json::json!(floor);
        self.conn.execute(
            "UPDATE folders SET sync_state_json = ?2 WHERE id = ?1",
            params![folder_id, v.to_string()],
        )?;
        Ok(())
    }

    /// The UIDNEXT this folder last reported — the precise any-new-mail test.
    pub fn folder_uidnext(&self, folder_id: i64) -> Result<Option<u32>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.get("uidnext").and_then(|m| m.as_u64()))
            .map(|v| v as u32))
    }

    /// Records the folder's last-seen UIDNEXT beside its other sync state.
    pub fn set_folder_uidnext(&mut self, folder_id: i64, uidnext: u32) -> Result<()> {
        let current: String = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "{}".into());
        let mut v: serde_json::Value =
            serde_json::from_str(&current).unwrap_or_else(|_| serde_json::json!({}));
        v["uidnext"] = serde_json::json!(uidnext);
        self.conn.execute(
            "UPDATE folders SET sync_state_json = ?2 WHERE id = ?1",
            params![folder_id, v.to_string()],
        )?;
        Ok(())
    }

    /// The HIGHESTMODSEQ this folder's flags were last reconciled at.
    pub fn folder_modseq(&self, folder_id: i64) -> Result<Option<u64>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v.get("modseq").and_then(|m| m.as_u64())))
    }

    /// Records the flag-reconciliation watermark alongside the folder's other
    /// sync state, without disturbing whatever else lives in that json.
    pub fn set_folder_modseq(&mut self, folder_id: i64, modseq: u64) -> Result<()> {
        let current: String = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "{}".into());
        let mut v: serde_json::Value =
            serde_json::from_str(&current).unwrap_or_else(|_| serde_json::json!({}));
        v["modseq"] = serde_json::json!(modseq);
        self.conn.execute(
            "UPDATE folders SET sync_state_json = ?2 WHERE id = ?1",
            params![folder_id, v.to_string()],
        )?;
        Ok(())
    }

    /// Clears the flag watermark — a renumbered folder's modseq domain is
    /// not comparable across the reset, so recovery starts flags over too.
    pub fn clear_folder_modseq(&mut self, folder_id: i64) -> Result<()> {
        let current: String = self
            .conn
            .query_row(
                "SELECT sync_state_json FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "{}".into());
        let mut v: serde_json::Value =
            serde_json::from_str(&current).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(map) = v.as_object_mut() {
            map.remove("modseq");
        }
        self.conn.execute(
            "UPDATE folders SET sync_state_json = ?2 WHERE id = ?1",
            params![folder_id, v.to_string()],
        )?;
        Ok(())
    }

    /// The message standing at this UID in this folder — the inverse of a
    /// placement, for code that has just fetched something and wants its row.
    pub fn message_id_at(&self, folder_id: i64, uid: u32) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT message_id FROM placements WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid as i64],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Applies a flag state reported by the server to whichever message sits
    /// at this UID in this folder. `false` when the UID is not one we hold —
    /// a flag change on mail outside the synced window is news about nothing.
    pub fn set_flags_by_uid(&mut self, folder_id: i64, uid: u32, flags: i64) -> Result<bool> {
        let message: Option<i64> = self
            .conn
            .query_row(
                "SELECT message_id FROM placements WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid as i64],
                |r| r.get(0),
            )
            .optional()?;
        match message {
            Some(id) => {
                self.set_message_flags(id, flags)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The server path of a folder, for addressing it over IMAP.
    pub fn folder_path(&self, folder_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT path FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}
