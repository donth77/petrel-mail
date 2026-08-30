//! Folders: places on the server, mirrored here.
//!
//! Moved verbatim from mod.rs (Phase 1.5 of the close-out plan) — a child
//! module sees the parent's private items, so `Store`'s fields and the
//! helpers stay where they were and nothing changed visibility to make
//! this split possible. Behavior lives in the tests, which did not move.
use super::*;

/// A literal string, made safe to use as the left side of a LIKE pattern.
///
/// SQLite's LIKE reads `%` and `_` as wildcards, and folder names contain
/// both — `glassdoor+102025_2` is an ordinary mailbox. Escaping with a
/// backslash, declared by `ESCAPE '\\'` at the call site, makes the pattern
/// mean the name.
fn like_escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Points the queue at a folder's new name.
///
/// A queued action carries the path its message sat at, captured when the
/// action was made so the change can still be delivered after a move has
/// deleted the placement that held the address. A rename turns that captured
/// path into a name the server no longer has, and the drain then fails with
/// `Mailbox doesn't exist` on every cycle, forever, carrying a change nobody
/// ever sees made — one such action had been retrying in a real mailbox since
/// the folder it named was moved to the Trash.
///
/// Renaming a folder renames the queue with it. Descendants come too, for the
/// same reason their folder rows do.
fn repoint_queued_actions(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    old_path: &str,
    new_path: &str,
) -> Result<()> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, payload_json FROM actions
             WHERE account_id = ?1 AND state = 'queued'",
        )?;
        let rows = stmt.query_map(params![account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, json) in rows {
        // A payload this version cannot read is left exactly as it is: the
        // queue holds work, and dropping work to tidy a path would be the
        // worse bug.
        let Ok(mut payload) = serde_json::from_str::<crate::actions::ActionPayload>(&json) else {
            continue;
        };
        let mut touched = false;
        for prior in &mut payload.prior {
            let Some(path) = prior.source_path.as_deref() else {
                continue;
            };
            let moved = if path == old_path {
                Some(new_path.to_string())
            } else {
                path.strip_prefix(old_path)
                    .filter(|rest| rest.starts_with(['/', '.']))
                    .map(|rest| format!("{new_path}{rest}"))
            };
            if let Some(moved) = moved {
                prior.source_path = Some(moved);
                touched = true;
            }
        }
        if touched && let Ok(next) = serde_json::to_string(&payload) {
            tx.execute(
                "UPDATE actions SET payload_json = ?2 WHERE id = ?1",
                params![id, next],
            )?;
        }
    }
    Ok(())
}

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
        // Nothing wears the role, so a folder already called by the role's own
        // name is adopted before one is invented. Namecheap marks no \Archive,
        // and the plain `Archive` sitting there is the archive by every
        // convention — it is what the rail already draws under the Archive
        // mailbox row, and where a decade of mail is filed.
        //
        // Inventing instead was silent data loss. `ensure_folder(_, "archive",
        // "archive")` made a local folder no server would ever list, the
        // archived message was placed only there, and the next folder survey
        // pruned it as a stranger — taking the message with it, out of every
        // view and out of search. One keystroke and a sync tick.
        let adopted: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM folders
                  WHERE account_id = ?1 AND coalesce(role,'') = ''
                    AND path = ?2 COLLATE NOCASE",
                params![account_id, path],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = adopted {
            self.conn.execute(
                "UPDATE folders SET role = ?2 WHERE id = ?1",
                params![id, role],
            )?;
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
             -- sort_order IS NULL sorts first in SQLite, which would put every
             -- folder nobody has dragged above the ones somebody arranged on
             -- purpose. The leading term flips that: arranged folders in the
             -- order chosen, then the untouched ones, still alphabetical.
             ORDER BY (role IS NOT NULL AND role <> ''),
                      (sort_order IS NULL), sort_order,
                      path COLLATE NOCASE",
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
                    // The server may set a role; it does not get to unset one.
                    // A survey reporting no special-use flag was wiping the
                    // role this app had assigned, one tick after assigning it.
                    self.conn.execute(
                        "UPDATE folders SET role = coalesce(?2, role), name = ?3
                          WHERE id = ?1",
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
        // The roles a server may simply never mark. Namecheap flags \Sent,
        // \Trash, \Drafts and \Junk but no \Archive, so the engine believed
        // the account had no archive at all: the Archive mailbox listed
        // nothing while ten thousand messages sat in the plain `Archive`
        // folder below it, and archiving invented a local folder rather than
        // filing into the real one.
        //
        // A top-level folder by the role's own name is that role by every
        // convention, and it is already what the rail draws and what the move
        // picker files into. Adopting it here is what makes the view, the
        // action and the drain agree with what the person can see.
        for (role, conventional) in [("archive", "Archive"), ("trash", "Trash")] {
            if self.folder_for_role(account_id, role)?.is_some() {
                continue;
            }
            self.conn.execute(
                "UPDATE folders SET role = ?3
                  WHERE account_id = ?1 AND coalesce(role,'') = ''
                    AND path = ?2 COLLATE NOCASE",
                params![account_id, conventional, role],
            )?;
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
                // Nor a folder wearing a role. Those are the app's own
                // structure rather than the server's listing, and one can
                // legitimately exist here before the server has been told —
                // an Archive created a moment ago, its message already in it,
                // the drain still queued. Pruning it there destroys the mail
                // in the gap between the two.
                .filter(|id| {
                    !self
                        .conn
                        .query_row(
                            "SELECT coalesce(role,'') <> '' FROM folders WHERE id = ?1",
                            params![id],
                            |r| r.get::<_, bool>(0),
                        )
                        .unwrap_or(false)
                })
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
        //
        // The pattern is escaped because a folder name is not a LIKE pattern.
        // An underscore matches any single character, so binning `a_b` also
        // matched `axb/child` and rewrote it to `Trash/a_b/child` — an
        // unrelated subtree dragged into the bin, onto a path the real child
        // already held. Mailboxes named `glassdoor+102025_2` are ordinary.
        let pattern = like_escape(&old_path);
        for delim in ['/', '.'] {
            tx.execute(
                "UPDATE folders
                 SET path = ?3 || substr(path, length(?2) + 1)
                 WHERE account_id = ?1 AND path LIKE ?5 || ?4 ESCAPE '\\'",
                params![account, old_path, new_path, format!("{delim}%"), pattern],
            )?;
        }
        repoint_queued_actions(&tx, account, &old_path, new_path)?;
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
    /// Removes a folder and, with it, mail that was only ever in it.
    ///
    /// Dropping the placements alone left those messages in the store with
    /// no folder at all: gone from every view, still answering searches, and
    /// reachable by no other route. That is the ghost state the inbox
    /// predicate was rewritten to avoid, arrived at from the other end — and
    /// after deleting a folder *out of the Trash*, where the word means what
    /// it says, it is also just wrong.
    ///
    /// So a message left with no placements is tombstoned exactly as
    /// delete-forever tombstones one: out of search now, bytes reaped by the
    /// grace-period sweep rather than here. Mail that lives somewhere else
    /// too — a Gmail message carrying other labels — keeps those placements
    /// and is untouched. Drafts and outbox rows are exempt: they are allowed
    /// to have no placement, and always were.
    ///
    /// Returns how many messages the folder took with it.
    pub fn remove_folder(&mut self, folder_id: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let orphans: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT m.id FROM messages m
                 JOIN placements p ON p.message_id = m.id AND p.folder_id = ?1
                 WHERE m.deleted_at_ms IS NULL
                   AND m.send_after_ms IS NULL
                   AND m.draft_msgid IS NULL
                   AND coalesce(m.draft_body, '') = ''
                   AND coalesce(m.draft_html, '') = ''
                   AND NOT EXISTS (SELECT 1 FROM placements q
                                    WHERE q.message_id = m.id AND q.folder_id != ?1)",
            )?;
            let rows = stmt.query_map(params![folder_id], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in &orphans {
            tx.execute(
                "UPDATE messages SET deleted_at_ms = (strftime('%s','now') * 1000)
                 WHERE id = ?1",
                params![id],
            )?;
            tx.execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
        }
        tx.execute(
            "DELETE FROM placements WHERE folder_id = ?1",
            params![folder_id],
        )?;
        tx.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
        tx.commit()?;
        Ok(orphans.len())
    }

    pub fn place_message(&self, message_id: i64, folder_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO placements(message_id, folder_id) VALUES (?1, ?2)",
            params![message_id, folder_id],
        )?;
        Ok(())
    }

    /// A folder and everything filed under it, by id.
    ///
    /// Every "all the mail in here" verb means the subtree, not the one
    /// mailbox: `Archive` itself holds a single message on a real account
    /// while `Archive/...` holds ten thousand, so a Mark all as read that
    /// stopped at the named folder would report marking one and look broken.
    /// Empty Trash already read the subtree; these follow it.
    ///
    /// Both separators, because a path is the server's and servers differ.
    /// Escaped, because a folder called `glassdoor+102025_2` is a name and not
    /// a LIKE pattern.
    pub fn folder_subtree(&self, folder_id: i64) -> Result<Vec<(i64, String)>> {
        let (account, path): (i64, String) = self.conn.query_row(
            "SELECT account_id, path FROM folders WHERE id = ?1",
            params![folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let pattern = like_escape(&path);
        let mut stmt = self.conn.prepare(
            "SELECT id, path FROM folders
              WHERE account_id = ?1
                AND (id = ?2
                     OR path LIKE ?3 || '/%' ESCAPE '\\'
                     OR path LIKE ?3 || '.%' ESCAPE '\\')
              ORDER BY path",
        )?;
        let rows = stmt.query_map(params![account, folder_id, pattern], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// How many messages a folder holds, for a sentence that names a number
    /// before somebody agrees to it.
    ///
    /// Messages, not conversations: "move 10,479 messages to the Trash" is
    /// what is about to happen, and a conversation count would understate it.
    pub fn folder_message_count(&self, folder_id: i64) -> Result<i64> {
        let ids = self.folder_subtree(folder_id)?;
        let holes = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        Ok(self.conn.query_row(
            &format!(
                "SELECT count(DISTINCT p.message_id) FROM placements p
                 JOIN messages m ON m.id = p.message_id
                 WHERE p.folder_id IN ({holes}) AND m.deleted_at_ms IS NULL"
            ),
            rusqlite::params_from_iter(ids.iter().map(|(id, _)| *id)),
            |r| r.get(0),
        )?)
    }

    /// Marks everything in a folder read, or unread — the local half.
    ///
    /// One statement rather than a loop over the placements: a folder can hold
    /// ten thousand messages, and this runs while somebody is looking at the
    /// sidebar waiting for the number to move.
    ///
    /// Returns how many rows actually changed, so "nothing to do" and "4,187
    /// marked" can be told apart in what the app says afterwards.
    pub fn mark_folder_seen(&self, folder_id: i64, seen: bool) -> Result<usize> {
        let (set, test) = if seen {
            (
                format!("flags | {}", flags::SEEN),
                format!("flags & {} = 0", flags::SEEN),
            )
        } else {
            (
                format!("flags & ~{}", flags::SEEN),
                format!("flags & {} != 0", flags::SEEN),
            )
        };
        let ids = self.folder_subtree(folder_id)?;
        let holes = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        Ok(self.conn.execute(
            &format!(
                "UPDATE messages SET flags = {set}
                 WHERE deleted_at_ms IS NULL AND {test}
                   AND id IN (SELECT message_id FROM placements
                               WHERE folder_id IN ({holes}))"
            ),
            rusqlite::params_from_iter(ids.iter().map(|(id, _)| *id)),
        )?)
    }

    /// Moves everything in a folder to another one — the local half of a
    /// "delete all", where the destination is the bin.
    ///
    /// Exclusive, like every other binning here: a message in the Trash is not
    /// also still sitting in the folder it came from, nor under any other
    /// label it happened to carry. That is what the word means on both kinds
    /// of provider, and the alternative is mail that reads as deleted and is
    /// still in three places.
    ///
    /// Returns how many messages moved.
    pub fn move_folder_contents(&mut self, folder_id: i64, to: i64) -> Result<usize> {
        let subtree = self.folder_subtree(folder_id)?;
        let holes = std::iter::repeat_n("?", subtree.len())
            .collect::<Vec<_>>()
            .join(",");
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT DISTINCT p.message_id FROM placements p
                 JOIN messages m ON m.id = p.message_id
                 WHERE p.folder_id IN ({holes}) AND m.deleted_at_ms IS NULL"
            ))?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(subtree.iter().map(|(id, _)| *id)),
                |r| r.get(0),
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        for id in &ids {
            tx.execute("DELETE FROM placements WHERE message_id = ?1", params![id])?;
            tx.execute(
                "INSERT OR IGNORE INTO placements(message_id, folder_id) VALUES (?1, ?2)",
                params![id, to],
            )?;
        }
        tx.commit()?;
        Ok(ids.len())
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

    /// Keeps `trashed_at_ms` honest: stamped when a message is first seen in
    /// the bin, cleared when it is no longer there.
    ///
    /// Maintained here rather than at the point of triage because mail
    /// reaches the bin by more routes than a click in this app — deleted on
    /// a phone, filed by a rule, moved by another client — and an expiry
    /// clock that only started for one of those routes would delete some
    /// mail early and keep the rest forever.
    pub fn refresh_trash_clock(&self, account_id: i64, now_ms: i64) -> Result<usize> {
        let in_trash = "EXISTS (SELECT 1 FROM placements p
                                JOIN folders f ON f.id = p.folder_id
                                WHERE p.message_id = messages.id
                                  AND f.account_id = ?1
                                  AND (f.role = 'trash'
                                       OR f.path LIKE (SELECT path || '/%' FROM folders
                                                        WHERE account_id = ?1 AND role = 'trash')
                                       OR f.path LIKE (SELECT path || '.%' FROM folders
                                                        WHERE account_id = ?1 AND role = 'trash')))";
        let started = self.conn.execute(
            &format!(
                "UPDATE messages SET trashed_at_ms = ?2
                 WHERE account_id = ?1 AND trashed_at_ms IS NULL AND {in_trash}"
            ),
            params![account_id, now_ms],
        )?;
        let cleared = self.conn.execute(
            &format!(
                "UPDATE messages SET trashed_at_ms = NULL
                 WHERE account_id = ?1 AND trashed_at_ms IS NOT NULL AND NOT {in_trash}"
            ),
            params![account_id],
        )?;
        Ok(started + cleared)
    }

    /// What has been in the bin longer than the given number of days, in the
    /// same shape `trash_contents` returns: the address the server needs.
    ///
    /// A message with no clock yet is never expired — better a bin that
    /// empties late than one that empties something it has not been watching.
    pub fn trash_expired(
        &self,
        account_id: i64,
        older_than_days: i64,
        now_ms: i64,
    ) -> Result<Vec<(String, u32, i64)>> {
        let cutoff =
            now_ms.saturating_sub(older_than_days.saturating_mul(crate::retention::MS_PER_DAY));
        Ok(self
            .trash_contents(account_id)?
            .into_iter()
            .filter(|(_, _, message_id)| {
                self.conn
                    .query_row(
                        "SELECT trashed_at_ms FROM messages WHERE id = ?1",
                        params![message_id],
                        |r| r.get::<_, Option<i64>>(0),
                    )
                    .ok()
                    .flatten()
                    .is_some_and(|t| t <= cutoff)
            })
            .collect())
    }

    /// Every UID sitting in this account's trash, with the folder path that
    /// holds it — what emptying the bin has to expunge on the server.
    ///
    /// Folders nested under the trash count too: dragging a folder to Trash
    /// puts it there, and a bin that quietly kept the mail inside its own
    /// subfolders would not be empty in any sense the word carries.
    pub fn trash_contents(&self, account_id: i64) -> Result<Vec<(String, u32, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, p.uid, p.message_id
             FROM placements p
             JOIN folders f ON f.id = p.folder_id
             WHERE f.account_id = ?1 AND p.uid IS NOT NULL
               AND (f.role = 'trash'
                    OR f.path LIKE (SELECT path || '/%' FROM folders
                                     WHERE account_id = ?1 AND role = 'trash')
                    OR f.path LIKE (SELECT path || '.%' FROM folders
                                     WHERE account_id = ?1 AND role = 'trash'))
             ORDER BY f.path, p.uid",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? as u32, r.get(2)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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

    /// Whether this account has that folder — still there, and its own.
    ///
    /// Rules and queued actions carry folder ids that outlive the folder: the
    /// user deletes it, the id stays written down. So anything about to file
    /// mail asks first.
    ///
    /// Ownership rather than bare existence because the id alone cannot tell
    /// the two apart, and the consequences differ only in how strange they
    /// look — a deleted folder strands the message, another account's folder
    /// files it next door. Every path that reaches here today scopes its
    /// folder list to the account already, so this is a floor rather than a
    /// fix: it is what makes offering one account's folders while another is
    /// on screen a bug that gets caught rather than mail that goes missing.
    pub fn account_owns_folder(&self, account_id: i64, folder_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM folders WHERE id = ?1 AND account_id = ?2",
                params![folder_id, account_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
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

    /// Records the order somebody dragged a set of rows into.
    ///
    /// Takes the ids in their new order and numbers them from zero. Renumbering
    /// the whole visible set rather than patching one row is what makes this
    /// safe to repeat: there are no gaps to run out of, no fractional indices
    /// to converge on a float, and a half-applied reorder cannot leave two rows
    /// claiming the same position.
    ///
    /// Rows not in the list keep whatever they had, so ordering one account's
    /// folders does not disturb another's, and a row that arrives from the
    /// server mid-drag simply stays unarranged until it is dragged too.
    ///
    /// `table` is not user input: the two callers pass a literal.
    fn set_order(&mut self, table: &'static str, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let sql = format!("UPDATE {table} SET sort_order = ?1 WHERE id = ?2");
            let mut stmt = tx.prepare(&sql)?;
            for (position, id) in ids.iter().enumerate() {
                stmt.execute(params![position as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The sidebar's folder order, as dragged.
    pub fn reorder_folders(&mut self, ids: &[i64]) -> Result<()> {
        self.set_order("folders", ids)
    }

    /// The sidebar's tag order, as dragged.
    pub fn reorder_tags(&mut self, ids: &[i64]) -> Result<()> {
        self.set_order("tags", ids)
    }
}
