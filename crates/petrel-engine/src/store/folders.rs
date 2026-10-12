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
pub(super) fn like_escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `sync_state_json` key on a folder made here that the server has not
/// confirmed yet. See `folder_awaits_server`.
const PENDING_CREATE: &str = "pending_create";

/// The separators a folder's children may be named with, given the
/// hierarchy delimiter its server reported (`set_folder_delimiters`).
///
/// IMAP names a child by its parent, the parent's delimiter and a leaf, and
/// each server has its own: `/` on Gmail, iCloud and Outlook, `.` on many
/// Courier and Dovecot hosts. Reading both everywhere made a folder honestly
/// called `Trash.2019` on a `/` server part of the bin — Empty Trash and the
/// bin's expiry expunged it — and dragged `Receipts.old` along whenever
/// `Receipts` was renamed, emptied or marked read. Once the server has said
/// which it uses, only that one nests. Before it has, both still do, which
/// is how every folder was read until it could.
pub(super) fn child_separators(delim: Option<&str>) -> Vec<char> {
    let mut one = delim.into_iter().flat_map(str::chars);
    match (one.next(), one.next()) {
        (Some(c), None) => vec![c],
        _ => vec!['/', '.'],
    }
}

/// Where a folder's name stands among the conventional names for its role,
/// lower first; any other name sorts after all of them. Compared on the
/// last segment, so `INBOX.Trash` and `[Gmail]/Sent Mail` read as their
/// leaves, and without regard to case — cPanel's spam folder is `spam`.
fn canonical_rank(role: &str, path: &str) -> usize {
    let names: &[&str] = match role {
        "inbox" => &["INBOX"],
        "sent" => &["Sent", "Sent Mail", "Sent Items", "Sent Messages"],
        "drafts" => &["Drafts", "Draft"],
        "trash" => &["Trash", "Deleted Items", "Deleted Messages", "Bin"],
        "spam" => &[
            "Spam",
            "Junk",
            "Junk E-mail",
            "Junk Email",
            "Junk Mail",
            "Bulk Mail",
        ],
        "archive" => &["Archive", "Archives", "All Mail"],
        "starred" => &["Starred", "Flagged"],
        _ => &[],
    };
    let leaf = path.rsplit(['/', '.']).next().unwrap_or(path);
    names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(leaf))
        .unwrap_or(names.len())
}

/// The `sync_state_json` key on a folder the server flags `\All` where that
/// is not the archive. See `set_all_mail_folders`.
const ALL_MAIL: &str = "all_mail";

/// The rows allowed to stand in no folder at all: a draft, pushed or not,
/// and post waiting in the outbox. Everything else with no placement is a
/// message the server has stopped holding. Written against the alias `m`.
pub(super) const NOT_DRAFT_OR_OUTBOX: &str = "m.send_after_ms IS NULL
                   AND m.draft_msgid IS NULL
                   AND coalesce(m.draft_body, '') = ''
                   AND coalesce(m.draft_html, '') = ''";

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
    separators: &[char],
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
                    .filter(|rest| rest.starts_with(separators))
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
        // A survey leaves one holder per role (`settle_roles`); the order is
        // for anything that reaches here before one has run.
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND role = ?2 ORDER BY id LIMIT 1",
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
            "INSERT INTO folders(account_id, role, name, path, sync_state_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account_id,
                role,
                path,
                path,
                self.born_state(account_id, None)?
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The `sync_state_json` a folder made here starts with: `key` set when
    /// given, and the account's delimiter (`account_delimiter`) when it has
    /// one, so the folder nests the way every folder the server listed does
    /// — a folder made before the next survey, or never listed at all, read
    /// both separators while the rail beside it read one.
    fn born_state(&self, account_id: i64, key: Option<&str>) -> Result<String> {
        let mut state = serde_json::Map::new();
        if let Some(key) = key {
            state.insert(key.into(), serde_json::Value::Bool(true));
        }
        if let Some(delim) = self.account_delimiter(account_id)? {
            state.insert("delim".into(), serde_json::Value::String(delim));
        }
        Ok(serde_json::Value::Object(state).to_string())
    }

    /// The delimiter this account's server nests with, as its survey recorded
    /// it on the folders it listed: the inbox's where the inbox has one.
    fn account_delimiter(&self, account_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT d FROM (
                   SELECT CASE WHEN json_valid(sync_state_json)
                               THEN json_extract(sync_state_json, '$.delim') END AS d,
                          path, id
                     FROM folders WHERE account_id = ?1)
                  WHERE length(d) = 1
                  ORDER BY upper(path) = 'INBOX' DESC, id
                  LIMIT 1",
                params![account_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Every folder on the account, for the move picker.
    ///
    /// Folders the user made come first, because those are what a move is
    /// usually for; the role folders are reachable from the rail already and
    /// are here only so V can get to them too.
    pub fn folders(&self, account_id: i64) -> Result<Vec<FolderSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, coalesce(role,''), path,
                    CASE WHEN json_valid(sync_state_json)
                         THEN json_extract(sync_state_json, '$.delim') END
               FROM folders
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
                delim: r
                    .get::<_, Option<String>>(3)
                    .ok()
                    .flatten()
                    .filter(|d| d.chars().count() == 1),
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
        // Who held which role before this survey said anything, for
        // `settle_roles`: the holder keeps its role over a newcomer.
        let held_before: std::collections::HashMap<i64, String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, role FROM folders
                  WHERE account_id = ?1 AND coalesce(role,'') <> ''",
            )?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
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
                    // Listed, so whatever was made here has arrived there.
                    self.confirm_folder_on_server(id)?;
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
        let listed: std::collections::HashSet<&str> =
            folders.iter().map(|(p, _)| p.as_str()).collect();
        self.settle_roles(account_id, &listed, &held_before)?;
        // The roles a server may simply never mark. Namecheap flags \Sent,
        // \Trash, \Drafts and \Junk but no \Archive, so the engine believed
        // the account had no archive at all: the Archive mailbox listed
        // nothing while ten thousand messages sat in the plain `Archive`
        // folder below it, and archiving invented a local folder rather than
        // filing into the real one. Other hosts skip \Sent or \Junk instead,
        // and then a send has nowhere to APPEND a copy, or reporting spam
        // has nowhere to file, even though `Sent` and `Junk` are sitting
        // right there.
        //
        // A top-level folder by a conventional name is that role by every
        // convention, and it is already what the rail draws and what the move
        // picker files into. Adopting it here is what makes the view, the
        // action and the drain agree with what the person can see. Nested
        // years (`Archive/2026`) keep their empty role: they are places, not
        // the mailbox. First matching name wins when several exist.
        const ADOPT: &[(&str, &[&str])] = &[
            ("archive", &["Archive"]),
            ("trash", &["Trash"]),
            (
                "sent",
                &["Sent", "Sent Items", "Sent Messages", "[Gmail]/Sent Mail"],
            ),
            (
                "spam",
                &[
                    "Spam",
                    "Junk",
                    "Junk E-mail",
                    "Junk Email",
                    "Junk Mail",
                    "[Gmail]/Spam",
                ],
            ),
        ];
        for (role, names) in ADOPT {
            if self.folder_for_role(account_id, role)?.is_some() {
                continue;
            }
            for name in *names {
                let n = self.conn.execute(
                    "UPDATE folders SET role = ?3
                      WHERE account_id = ?1 AND coalesce(role,'') = ''
                        AND path = ?2 COLLATE NOCASE",
                    params![account_id, *name, role],
                )?;
                if n > 0 {
                    break;
                }
            }
        }

        // Folders the server no longer lists are gone — renamed elsewhere,
        // deleted elsewhere, or (the day this was written) a \Noselect
        // container that stopped being reported as a mailbox. Their rows and
        // placements go; their mail stays, as it does everywhere else here.
        let stale: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, path FROM folders WHERE account_id = ?1")?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.filter_map(|r| r.ok())
                .filter(|(_, path)| !listed.contains(path.as_str()))
                .map(|(id, _)| id)
                // A local folder is absent from every survey by definition —
                // imported mail lives in one, and pruning it would delete the
                // only placements that mail has.
                .filter(|id| !self.folder_is_local(*id).unwrap_or(false))
                // Nor one made here that the server has not got yet. Its
                // absence means "not created there yet", not "deleted there":
                // the create runs in the background and a survey can land in
                // the gap, and a create that failed is retried by the next
                // sync. Pruning either deleted the folder the person had just
                // made, with no word to say it had gone.
                .filter(|id| !self.folder_awaits_server(*id).unwrap_or(false))
                // Nor a folder still wearing a role. Those are the app's own
                // structure rather than the server's listing, and one can
                // legitimately exist here before the server has been told —
                // an Archive created a moment ago, its message already in it,
                // the drain still queued. Pruning it there destroys the mail
                // in the gap between the two. A role folder the server has
                // replaced lost its role in `settle_roles` above, and goes
                // here like any other folder the server no longer lists.
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
        // Mirror mode follows the server: the folder's mail is tombstoned
        // with the usual grace, and comes back by itself if the folder was
        // only renamed (the re-ingest under the new name clears the
        // tombstone). A local archive promises the opposite of following
        // the server, so there the folder row and its placements go and
        // the messages stay exactly as they were.
        let keep_mail =
            self.retention_mode(account_id)? == crate::retention::RetentionMode::LocalArchive;
        for id in stale {
            if keep_mail {
                self.forget_folder(id)?;
            } else {
                self.remove_folder(id)?;
            }
        }
        Ok(n)
    }

    /// Leaves one folder holding each role.
    ///
    /// A server may flag several mailboxes with one special use. cPanel's
    /// Dovecot flags its own `Trash` and Apple's `Deleted Messages` both
    /// `\Trash`, `Spam` and `Junk` both `\Junk`, and `Sent`, `Sent Items` and
    /// `Sent Messages` all `\Sent`. With all of them wearing the role, the
    /// sync took the first in rail order and triage the lowest id: on a real
    /// account the Trash being synced was a `Deleted Messages` the server had
    /// deleted weeks before, while the real Trash and Spam were reached only
    /// by the twenty-minute reconcile.
    ///
    /// So each role has one holder, the same for every reader, chosen in this
    /// order:
    /// 1. a folder this survey listed, over one it did not;
    /// 2. the folder that held the role before the survey — so emptying the
    ///    Trash never hands the role to the folder beside it that still has
    ///    mail in it;
    /// 3. a folder already holding mail synced from the server;
    /// 4. the conventional name (`canonical_rank`);
    /// 5. the lowest id, which is the order the server listed them.
    ///
    /// The others become ordinary folders. Their mail stays placed and syncs
    /// as any folder's does, as Thunderbird lists every one of them.
    ///
    /// Only folders this survey listed, local folders and folders made here
    /// that the server has not confirmed are candidates. A role holder that
    /// is none of those — the server's folder is gone, or the role was
    /// invented here before any survey — keeps the role only while nothing
    /// else can take it. Once something can, it gives way, and whatever was
    /// filed into it here without a server number goes to the new holder
    /// first: that is mail waiting for the drain, and pruning the folder with
    /// it still inside would take it out of every view.
    ///
    /// Outside Gmail a mailbox the server flags `\All` is a view of every
    /// message, which the last survey marked (`set_all_mail_folders`). It
    /// never holds the archive: archiving into it is a move the server
    /// cannot make, while the plain Archive beside it went unused.
    fn settle_roles(
        &self,
        account_id: i64,
        listed: &std::collections::HashSet<&str>,
        held_before: &std::collections::HashMap<i64, String>,
    ) -> Result<()> {
        let gmail: bool = self
            .conn
            .query_row(
                "SELECT kind = 'gmail' FROM accounts WHERE id = ?1",
                params![account_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let holders: Vec<(i64, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, role, path FROM folders
                  WHERE account_id = ?1 AND coalesce(role,'') <> ''
                  ORDER BY id",
            )?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut by_role: std::collections::BTreeMap<String, Vec<(i64, String)>> =
            std::collections::BTreeMap::new();
        for (id, role, path) in holders {
            if role == "archive" && !gmail && self.folder_is_all_mail(id)? {
                self.conn
                    .execute("UPDATE folders SET role = NULL WHERE id = ?1", params![id])?;
                continue;
            }
            by_role.entry(role).or_default().push((id, path));
        }
        for (role, group) in by_role {
            if group.len() < 2 {
                continue;
            }
            // (id, path, listed, may stand unlisted)
            let mut scored = Vec::with_capacity(group.len());
            for (id, path) in group {
                let is_listed = listed.contains(path.as_str());
                let kept_here = self.folder_is_local(id)? || self.folder_awaits_server(id)?;
                scored.push((id, path, is_listed, kept_here));
            }
            let mut candidates = Vec::new();
            for (id, path, is_listed, kept_here) in &scored {
                if !(*is_listed || *kept_here) {
                    continue;
                }
                let has_mail: bool = self.conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM placements
                                     WHERE folder_id = ?1 AND uid IS NOT NULL)",
                    params![id],
                    |r| r.get(0),
                )?;
                let held = held_before.get(id).is_some_and(|r| *r == role);
                // Larger is better; `max_by_key` keeps the last of equals, so
                // the id is negated to make the lowest win.
                let key = (
                    *is_listed,
                    held,
                    has_mail,
                    std::cmp::Reverse(canonical_rank(&role, path)),
                    std::cmp::Reverse(*id),
                );
                candidates.push((key, *id));
            }
            let Some(winner) = candidates.iter().max_by_key(|(k, _)| *k).map(|(_, id)| *id) else {
                // Nothing the server lists and nothing made to stay here:
                // the role stays where it was until a survey lists a folder
                // that can take it.
                continue;
            };
            for (id, _, is_listed, kept_here) in scored {
                if id == winner {
                    continue;
                }
                self.conn
                    .execute("UPDATE folders SET role = NULL WHERE id = ?1", params![id])?;
                if !is_listed && !kept_here {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO placements(message_id, folder_id)
                         SELECT message_id, ?2 FROM placements
                          WHERE folder_id = ?1 AND uid IS NULL",
                        params![id, winner],
                    )?;
                    self.conn.execute(
                        "DELETE FROM placements WHERE folder_id = ?1 AND uid IS NULL",
                        params![id],
                    )?;
                }
            }
        }
        Ok(())
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
        // Born waiting for the server. The create there runs after this
        // returns, and until something confirms it the folder is protected
        // from the survey and retried by the sync. See `folder_awaits_server`.
        self.conn.execute(
            "INSERT INTO folders(account_id, role, name, path, sync_state_json)
             VALUES (?1, NULL, ?2, ?3, ?4)",
            params![
                account_id,
                name,
                path,
                self.born_state(account_id, Some(PENDING_CREATE))?
            ],
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
        let separators = self.folder_separators(folder_id)?;
        let name = new_path
            .rsplit(separators.as_slice())
            .next()
            .unwrap_or(new_path);
        let tx = self.conn.transaction()?;
        // The server renames a subtree in one RENAME (RFC 3501); the local
        // tree follows suit, or every descendant's path goes quietly stale
        // and the next survey prunes real folders as strangers.
        //
        // One statement, so each row is judged once and by its old path, and
        // never the folder itself. Run per separator after the folder's own
        // update, renaming `Taxes` to `Taxes.old` found the renamed row under
        // `Taxes.` and made it `Taxes.old.old` here while the server had
        // `Taxes.old`. Compared exactly rather than with LIKE, which ignores
        // ASCII case — renaming `Work` also took `work/Private`, a different
        // folder — and reads `_` and `%` in a name as wildcards: binning
        // `a_b` once took `axb/child` along. Mailboxes named
        // `glassdoor+102025_2` are ordinary.
        let (first, last) = (separators[0], separators[separators.len() - 1]);
        tx.execute(
            "UPDATE folders
                SET path = ?3 || substr(path, length(?2) + 1)
              WHERE account_id = ?1 AND id <> ?4
                AND substr(path, 1, length(?2) + 1) IN (?2 || ?5, ?2 || ?6)",
            params![
                account,
                old_path,
                new_path,
                folder_id,
                first.to_string(),
                last.to_string()
            ],
        )?;
        tx.execute(
            "UPDATE folders SET path = ?2, name = ?3 WHERE id = ?1",
            params![folder_id, new_path, name],
        )?;
        repoint_queued_actions(&tx, account, &old_path, new_path, &separators)?;
        tx.commit()?;
        // And the saved searches that name it — by leaf, by whole path, or as a
        // descendant the rename carried with it. After the commit rather than
        // inside it: the worst a failure here leaves is a query holding the old
        // name, which is exactly where it stood before any of this existed.
        self.rename_in_saved_searches(
            account,
            &crate::search_query::Renamed::Folder {
                from: &old_path,
                to: new_path,
                separators: &separators,
            },
        )?;
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
    /// Drops a folder row and its placements and nothing else: the messages
    /// stay, untombstoned, with whatever other placements they have. For a
    /// folder the server stopped listing on an account that keeps a local
    /// archive — the case the archive exists for.
    pub fn forget_folder(&mut self, folder_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM placements WHERE folder_id = ?1",
            params![folder_id],
        )?;
        tx.execute("DELETE FROM folders WHERE id = ?1", params![folder_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Returns how many messages the folder took with it.
    pub fn remove_folder(&mut self, folder_id: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let orphans: Vec<i64> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT m.id FROM messages m
                 JOIN placements p ON p.message_id = m.id AND p.folder_id = ?1
                 WHERE m.deleted_at_ms IS NULL
                   AND {NOT_DRAFT_OR_OUTBOX}
                   AND NOT EXISTS (SELECT 1 FROM placements q
                                    WHERE q.message_id = m.id AND q.folder_id != ?1)"
            ))?;
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
    /// By the separator the server nests with (`child_separators`): on a `/`
    /// server `Receipts.old` is a sibling, and emptying or marking `Receipts`
    /// used to reach into it on the server. Compared exactly, not with LIKE,
    /// which ignores case and reads `_` in `glassdoor+102025_2` as a wildcard.
    pub fn folder_subtree(&self, folder_id: i64) -> Result<Vec<(i64, String)>> {
        let (account, path): (i64, String) = self.conn.query_row(
            "SELECT account_id, path FROM folders WHERE id = ?1",
            params![folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let separators = self.folder_separators(folder_id)?;
        let (first, last) = (separators[0], separators[separators.len() - 1]);
        let mut stmt = self.conn.prepare(
            "SELECT id, path FROM folders
              WHERE account_id = ?1
                AND (id = ?2
                     OR substr(path, 1, length(?3) + 1) IN (?3 || ?4, ?3 || ?5))
              ORDER BY path",
        )?;
        let rows = stmt.query_map(
            params![
                account,
                folder_id,
                path,
                first.to_string(),
                last.to_string()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
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

    /// Whether this message currently sits in a folder of the given role.
    pub fn message_in_role(&self, message_id: i64, role: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM placements p
               JOIN folders f ON f.id = p.folder_id
               WHERE p.message_id = ?1 AND f.role = ?2
             )",
            params![message_id, role],
            |r| r.get::<_, i64>(0),
        )? == 1)
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

    /// The folder this account holds at `path`, and the UIDVALIDITY its stored
    /// numbers belong to.
    ///
    /// What a UID kept or found for that path is checked against before
    /// anything acts on it, and what a log names the folder by.
    ///
    /// The validity read here pairs only with a number read from a placement
    /// under the same guard. Recovery records the new validity only after it
    /// has re-numbered the placements, so that reader can meet a new number
    /// under the old validity (refused by the server's SELECT, and asked
    /// again) but never an old number under the new one. A number held
    /// anywhere else goes with the validity it was learned under, and is
    /// checked against that: an action's captured source
    /// (`pending_actions`), the one undo puts back (`undo_action`), one a
    /// search found (`heal_placement_uid`). Paired with the validity as it
    /// stands, each named another message once recovery had run (review r1).
    pub fn folder_at(&self, account_id: i64, path: &str) -> Result<Option<(i64, Option<u32>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, uidvalidity FROM folders WHERE account_id = ?1 AND path = ?2",
                params![account_id, path],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?
            .map(|(id, validity)| (id, validity.map(|v| v as u32))))
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
    /// A message a complete listing leaves with no placement at all is
    /// tombstoned as the ghost sweep would tombstone it — a soft delete with
    /// the usual grace, cleared by a re-ingest — never deleted outright.
    pub fn remap_folder_after_reset(
        &mut self,
        folder_id: i64,
        server: &[(u32, Option<String>)],
        complete: bool,
    ) -> Result<RemapOutcome> {
        // Placements that had no number before the reset are local or
        // quarantined already; the listing says nothing about them, so they
        // are not the server's word that the message is gone.
        let unaddressed_before: std::collections::HashSet<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT message_id FROM placements WHERE folder_id = ?1 AND uid IS NULL",
            )?;
            let rows = stmt.query_map(params![folder_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let tx = self.conn.unchecked_transaction()?;
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
        // Matched only where the listing cannot be wrong about which row a
        // number belongs to (`claims_by_message_id`). Two messages under one
        // Message-ID — two rows, or one id listed twice — are fetched again
        // and placed by their bytes; matching by the id handed the plain row
        // the other's number and hid the other (docs/25 review).
        let claims = self.claims_by_message_id(account, server)?;
        for (uid, claim) in claims {
            let matched = match claim {
                Some(holder) => tx.execute(
                    "UPDATE placements SET uid = ?1 WHERE folder_id = ?2 AND message_id = ?3",
                    params![uid as i64, folder_id, holder],
                )?,
                // No Message-ID on the wire: indistinguishable from mail we
                // have (whose ingest key was a blob hash), so refetch it and
                // let ingest's own dedupe decide. Likewise an id more than one
                // message carries.
                None => 0,
            };
            if matched > 0 {
                out.rematched += 1;
            } else {
                out.to_fetch.push(uid);
            }
        }
        if complete {
            let dropping: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "SELECT message_id FROM placements WHERE folder_id = ?1 AND uid IS NULL",
                )?;
                let rows = stmt.query_map(params![folder_id], |r| r.get::<_, i64>(0))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            out.dropped = tx.execute(
                "DELETE FROM placements WHERE folder_id = ?1 AND uid IS NULL",
                params![folder_id],
            )?;
            let proven_gone: Vec<i64> = dropping
                .into_iter()
                .filter(|id| !unaddressed_before.contains(id))
                .collect();
            self.tombstone_unplaced(folder_id, &proven_gone)?;
        }
        // The copy a draft's last push recorded is a number in Drafts too,
        // and as much a lie now. It follows the number the re-map gave the
        // draft's own placement, or goes: kept as it was, the removal of a
        // sent draft's server copies expunged whatever message had inherited
        // it, under the folder's new validity.
        tx.execute(
            "UPDATE messages SET draft_server_uid =
                 (SELECT p.uid FROM placements p
                   WHERE p.message_id = messages.id AND p.folder_id = ?1)
              WHERE account_id = ?2 AND draft_server_uid IS NOT NULL
                AND EXISTS (SELECT 1 FROM folders WHERE id = ?1 AND role = 'drafts')",
            params![folder_id, account],
        )?;
        tx.commit()?;
        Ok(out)
    }

    /// The message this account holds under a wire Message-ID, if any.
    /// Writes a UID the server has just confirmed onto the one placement that
    /// lost its number — and only onto one that lost it. A placement already
    /// holding a UID belongs to the sync; the drain never overrules it.
    ///
    /// And only while the folder's UIDVALIDITY is still `validity`, the one
    /// read before the server was asked. Recovery can finish in between, and
    /// a number from before its renumbering, written after it, pairs with
    /// the new validity and passes the server's check against some other
    /// message (review r1, R1-8).
    pub fn heal_placement_uid(
        &self,
        message_id: i64,
        account_id: i64,
        folder_path: &str,
        uid: u32,
        validity: Option<u32>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE placements SET uid = ?1
             WHERE message_id = ?2 AND uid IS NULL
               AND folder_id = (SELECT id FROM folders
                                 WHERE account_id = ?3 AND path = ?4
                                   AND uidvalidity IS ?5)",
            params![
                uid as i64,
                message_id,
                account_id,
                folder_path,
                validity.map(|v| v as i64)
            ],
        )?;
        Ok(n > 0)
    }

    /// How many placements in this folder claim a server UID — the number a
    /// folder's STATUS should agree with, ghosts aside.
    /// Placements in a folder that carry no UID yet: what a label sweep makes
    /// when it files by Message-ID, and what a full listing of the folder can
    /// number. Zero means that listing has nothing to do.
    pub fn unnumbered_placement_count(&self, folder_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM placements WHERE folder_id = ?1 AND uid IS NULL",
            params![folder_id],
            |r| r.get(0),
        )?)
    }

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
    ///
    /// A message this leaves with no placement at all is tombstoned, as
    /// deleting its folder would tombstone it — see `tombstone_unplaced` for
    /// when that holds. Dropping the placement alone made a permanent
    /// ghost: out of every view, still answering searches, still in its
    /// conversation, and never collected.
    pub fn remove_placements_absent(
        &self,
        folder_id: i64,
        present: &std::collections::HashSet<u32>,
    ) -> Result<usize> {
        let stored: Vec<(i64, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT message_id, uid FROM placements WHERE folder_id = ?1 AND uid IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let gone: Vec<(i64, i64)> = stored
            .into_iter()
            .filter(|(_, u)| !present.contains(&(*u as u32)))
            .collect();
        if gone.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        for chunk in gone.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&folder_id];
            for (_, u) in chunk {
                params.push(u);
            }
            tx.execute(
                &format!("DELETE FROM placements WHERE folder_id = ?1 AND uid IN ({marks})"),
                rusqlite::params_from_iter(params),
            )?;
        }
        let ids: Vec<i64> = gone.iter().map(|(m, _)| *m).collect();
        self.tombstone_unplaced(folder_id, &ids)?;
        tx.commit()?;
        Ok(gone.len())
    }

    /// Tombstones whichever of `ids` the server has just stopped holding
    /// anywhere, the way `remove_folder` does: out of search now, bytes
    /// reaped by the grace-period sweep, and cleared again by a re-ingest
    /// should the message turn up in another folder. `folder_id` is the
    /// folder whose placements were just removed.
    ///
    /// Only for an account that mirrors the server: a local archive keeps
    /// exactly this mail, on purpose. And only where leaving the folder means
    /// leaving the server. Where folders are labels, mail archived on the
    /// phone drops out of INBOX and is still in All Mail, which the walk may
    /// not have claimed yet; a label removed is a label removed. There, only
    /// the bins and All Mail itself can say a message is gone. Drafts and
    /// outbox rows are never touched: they are allowed to have no placement.
    fn tombstone_unplaced(&self, folder_id: i64, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let Some((account, role)) = self
            .conn
            .query_row(
                "SELECT account_id, coalesce(role, '') FROM folders WHERE id = ?1",
                params![folder_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        else {
            return Ok(0);
        };
        if self.retention_mode(account)? == crate::retention::RetentionMode::LocalArchive {
            return Ok(0);
        }
        let means_gone = match self.placement_policy(account)? {
            crate::actions::PlacementPolicy::Exclusive => true,
            crate::actions::PlacementPolicy::Labels => {
                matches!(role.as_str(), "archive" | "trash" | "spam")
            }
        };
        if !means_gone {
            return Ok(0);
        }
        let mut tombstoned = 0usize;
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let orphans: Vec<i64> = {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT m.id FROM messages m
                     WHERE m.id IN ({marks}) AND m.deleted_at_ms IS NULL
                       AND {NOT_DRAFT_OR_OUTBOX}
                       AND NOT EXISTS (SELECT 1 FROM placements q WHERE q.message_id = m.id)"
                ))?;
                let rows =
                    stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get(0))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            for id in orphans {
                self.conn.execute(
                    "UPDATE messages SET deleted_at_ms = (strftime('%s','now') * 1000)
                     WHERE id = ?1",
                    params![id],
                )?;
                self.conn
                    .execute("DELETE FROM fts_content WHERE message_id = ?1", params![id])?;
                tombstoned += 1;
            }
        }
        Ok(tombstoned)
    }

    /// Gives the folder's unnumbered placements the numbers a listing of it
    /// holds, and drops the ones the listing does not hold.
    ///
    /// `server` is the folder's complete `(UID, Message-ID)` listing. The
    /// placements in question are the ones `remove_placements_absent` has to
    /// leave alone because they carry no number: a Gmail label sweep files
    /// into the inbox by Message-ID and learns no INBOX number, and a move
    /// files locally before the server has assigned one. Matched against the
    /// listing by the key ingest dedupes on, a placement either learns its
    /// number or is proven absent — and an absent one goes, as an absent
    /// numbered one would, tombstoning the message if that was its last
    /// folder and the account mirrors the server.
    ///
    /// Never a placement whose message has work queued, nor one touched by
    /// an action in the last few minutes: a listing taken a moment before
    /// the drain delivered a move is a lie about exactly that placement, and
    /// dropping it would undo the move on screen. Drafts and outbox rows are
    /// the store's own and are not the server's to prune.
    pub fn reconcile_unaddressed_placements(
        &self,
        folder_id: i64,
        server: &[(u32, Option<String>)],
    ) -> Result<RemapOutcome> {
        const RECENT_MS: i64 = 10 * 60 * 1000;
        let mut out = RemapOutcome::default();
        if self.folder_is_local(folder_id)? {
            return Ok(out);
        }
        // Each Message-ID the listing holds, with every UID it is listed at.
        let mut listed: std::collections::HashMap<&str, Vec<u32>> =
            std::collections::HashMap::new();
        for (uid, mid) in server {
            if let Some(m) = mid.as_deref() {
                listed.entry(m).or_default().push(*uid);
            }
        }
        let unaddressed: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT p.message_id, coalesce(m.message_id_hdr, '')
                 FROM placements p
                 JOIN messages m ON m.id = p.message_id
                 WHERE p.folder_id = ?1 AND p.uid IS NULL
                   AND {NOT_DRAFT_OR_OUTBOX}
                   AND NOT EXISTS (SELECT 1 FROM action_messages am
                                   JOIN actions a ON a.id = am.action_id
                                   WHERE am.message_id = m.id
                                     AND (a.state = 'queued'
                                          OR a.created_ms > (strftime('%s','now') * 1000) - ?2))"
            ))?;
            let rows = stmt.query_map(params![folder_id, RECENT_MS], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        if unaddressed.is_empty() {
            return Ok(out);
        }
        let account: i64 = self.conn.query_row(
            "SELECT account_id FROM folders WHERE id = ?1",
            params![folder_id],
            |r| r.get(0),
        )?;
        // The numbers already given to rows in this folder: a listed copy
        // under one of them is that row's, not a candidate for another.
        let numbered: std::collections::HashSet<u32> =
            self.placement_uids(folder_id)?.into_iter().collect();
        let tx = self.conn.unchecked_transaction()?;
        let mut dropped: Vec<i64> = Vec::new();
        for (message_id, key) in unaddressed {
            // The copies listed under the row's wire id that no other row in
            // this folder has a number for. None, and the folder holds no
            // copy that can be this one: the placement goes, as before. One,
            // and only this row carries the id under its plain key: that is
            // its number, as before. Anything else — two rows that could be
            // the copy, the id listed twice, a row under a suffixed key — the
            // listing cannot settle: the placement stays unnumbered, the copy
            // is fetched by the folder's own sync, and ingest places it by its
            // bytes. Matching by the plain key alone gave the plain row a
            // copy that was another message's, and dropped every row under a
            // suffixed key as absent (docs/25 review).
            let free: Vec<u32> = super::wire_message_id(&key)
                .and_then(|m| listed.get(m))
                .map(|uids| {
                    uids.iter()
                        .copied()
                        .filter(|u| !numbered.contains(u))
                        .collect()
                })
                .unwrap_or_default();
            if free.is_empty() {
                tx.execute(
                    "DELETE FROM placements WHERE message_id = ?1 AND folder_id = ?2",
                    params![message_id, folder_id],
                )?;
                dropped.push(message_id);
                continue;
            }
            let wire = super::wire_message_id(&key).unwrap_or_default();
            let only_this_row = listed.get(wire).is_some_and(|uids| uids.len() == 1)
                && self.sole_holder_of(account, wire)? == Some(message_id);
            if only_this_row {
                tx.execute(
                    "UPDATE placements SET uid = ?3 WHERE message_id = ?1 AND folder_id = ?2",
                    params![message_id, folder_id, free[0] as i64],
                )?;
                out.rematched += 1;
            } else {
                out.to_fetch.extend(free);
            }
        }
        out.dropped = dropped.len();
        self.tombstone_unplaced(folder_id, &dropped)?;
        tx.commit()?;
        Ok(out)
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
        let in_trash = format!(
            "EXISTS (SELECT 1 FROM placements p
                     JOIN folders f ON f.id = p.folder_id
                     WHERE p.message_id = messages.id
                       AND f.account_id = ?1
                       AND (f.role = 'trash' OR f.id IN ({TRASH_SUBFOLDER_IDS})))"
        );
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
        let mut stmt = self.conn.prepare(&format!(
            "SELECT f.path, p.uid, p.message_id
             FROM placements p
             JOIN folders f ON f.id = p.folder_id
             WHERE f.account_id = ?1 AND p.uid IS NOT NULL
               AND (f.role = 'trash' OR f.id IN ({TRASH_SUBFOLDER_IDS}))
             ORDER BY f.path, p.uid"
        ))?;
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

    /// The one row a server copy known only by its Message-ID can be.
    ///
    /// Some paths learn of a server message from a listing — its UID and its
    /// Message-ID, no bytes: a folder renumbered by a UIDVALIDITY reset, the
    /// Gmail label sweep, the numbering of the inbox placements that sweep
    /// makes, and the All Mail walk. Since a different message under a stored
    /// one's Message-ID became a row of its own (docs/25 #78), more than one
    /// row can carry an id, and handing a copy to the plain row gave one
    /// message another's number: a renumbered folder hid the newer of two
    /// scans and bound its server copy to the older, so deleting the older
    /// deleted the newer on the server (docs/25 review).
    ///
    /// So this answers only where the answer cannot be wrong: exactly one
    /// live row in the account carries the id, and it is the row under the
    /// plain key — the case that has always matched. Otherwise `None`, and
    /// the caller fetches the copy and lets ingest place it by its bytes.
    pub fn sole_holder_of(&self, account_id: i64, msgid: &str) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT m.id, m.message_id_hdr = ?2 FROM messages m
              WHERE m.account_id = ?1 AND m.deleted_at_ms IS NULL AND {}
              LIMIT 2",
            super::CARRIES_WIRE_ID
        ))?;
        let rows: Vec<(i64, bool)> = stmt
            .query_map(params![account_id, msgid], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(match rows.as_slice() {
            [(id, true)] => Some(*id),
            _ => None,
        })
    }

    /// Whether no other live row carries this message's Message-ID: no
    /// different message kept under a content key, and no second copy.
    ///
    /// Then a server copy carrying the exact id can only be this message's
    /// own, whatever its size — and the size is not a fair test for it,
    /// because one row stands for every copy of the same message (sent to
    /// yourself, delivered under an alias) and keeps the bytes of one of
    /// them. The drain asks this before it refuses a copy for its size
    /// (docs/25 review). A message with no Message-ID carries nothing
    /// anyone could share.
    pub fn carries_its_message_id_alone(&self, message_id: i64) -> Result<bool> {
        let row: Option<(i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT account_id, message_id_hdr FROM messages WHERE id = ?1",
                params![message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((account_id, Some(key))) = row else {
            return Ok(true);
        };
        let Some(wire) = super::wire_message_id(&key) else {
            return Ok(true);
        };
        let others: i64 = self.conn.query_row(
            &format!(
                "SELECT count(*) FROM messages m
                  WHERE m.account_id = ?1 AND m.deleted_at_ms IS NULL AND m.id != ?3
                    AND {}",
                super::CARRIES_WIRE_ID
            ),
            params![account_id, wire, message_id],
            |r| r.get(0),
        )?;
        Ok(others == 0)
    }

    /// Whether these bytes are this message by every field the identity
    /// reads (`petrel_mime::feed_identity`): the same message as the row,
    /// whatever trace headers the copy carries. False when either side
    /// cannot be read.
    pub fn is_same_message(
        &self,
        blobs: &crate::blob::BlobStore,
        message_id: i64,
        raw: &[u8],
    ) -> Result<bool> {
        let Some(hash) = self.blob_hash_for(message_id)? else {
            return Ok(false);
        };
        let Ok(stored) = blobs.read(&hash) else {
            return Ok(false);
        };
        Ok(
            match (
                super::content_identity(&stored),
                super::content_identity(raw),
            ) {
                (Some((a, _)), Some((b, _))) => a == b,
                _ => false,
            },
        )
    }

    /// The row each listed server copy may be claimed for without fetching
    /// it, in the listing's order: `Some` only where `sole_holder_of` answers
    /// and the listing names the id once. A Message-ID listed twice is two
    /// messages on the server, and the listing cannot say which is which.
    pub fn claims_by_message_id(
        &self,
        account_id: i64,
        listed: &[(u32, Option<String>)],
    ) -> Result<Vec<(u32, Option<i64>)>> {
        let mut times: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for mid in listed.iter().filter_map(|(_, m)| m.as_deref()) {
            *times.entry(mid).or_default() += 1;
        }
        let mut out = Vec::with_capacity(listed.len());
        for (uid, mid) in listed {
            let row = match mid.as_deref() {
                Some(m) if times.get(m) == Some(&1) => self.sole_holder_of(account_id, m)?,
                _ => None,
            };
            out.push((*uid, row));
        }
        Ok(out)
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
        self.folder_flag(folder_id, "local")
    }

    /// Whether a folder made here is still waiting for the server to have it.
    ///
    /// Set when the user names a new folder, and cleared by the first survey
    /// that lists it or by a create the server confirmed. Until then the
    /// server has never heard of it, so renaming or deleting it there would
    /// ask about a mailbox that does not exist — those stay local — and its
    /// absence from a survey means "not made yet", not "deleted elsewhere".
    pub fn folder_awaits_server(&self, folder_id: i64) -> Result<bool> {
        self.folder_flag(folder_id, PENDING_CREATE)
    }

    /// The folders made here that the server does not have yet, as
    /// `(id, path)`, parents before children so each create has somewhere to
    /// go. Local folders are not in it: they are never meant to leave.
    pub fn folders_awaiting_server(&self, account_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, path FROM folders
              WHERE account_id = ?1
                AND json_valid(sync_state_json)
                AND json_extract(sync_state_json, '$.pending_create') IN (1, 'true')
                AND coalesce(json_extract(sync_state_json, '$.local'), 0) NOT IN (1, 'true')
              ORDER BY length(path), path",
        )?;
        let rows = stmt.query_map(params![account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The server confirmed it has this folder: it is an ordinary one now.
    pub fn confirm_folder_on_server(&self, folder_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET sync_state_json = json_remove(sync_state_json, '$.pending_create')
              WHERE id = ?1 AND json_valid(sync_state_json)",
            params![folder_id],
        )?;
        Ok(())
    }

    /// Records the hierarchy delimiter the server reported for each folder,
    /// which is what `child_separators` nests by. `delims` pairs a path with
    /// the delimiter its LIST line carried; a folder not named keeps what it
    /// had, or takes the account's if it had none. A delimiter that is not
    /// one character (NIL, on a flat server) is not one this can nest by, and
    /// is forgotten rather than guessed at.
    pub fn set_folder_delimiters(
        &mut self,
        account_id: i64,
        delims: &[(String, Option<String>)],
    ) -> Result<()> {
        let one = |d: &Option<String>| d.clone().filter(|d| d.chars().count() == 1);
        // The account's: what the inbox was listed with, or failing that the
        // first folder that said. Folders the survey did not name — made here
        // and never listed, or listed before delimiters were kept — take it
        // when they have none, so every folder of the account nests alike.
        let account = delims
            .iter()
            .find(|(path, _)| path.eq_ignore_ascii_case("INBOX"))
            .and_then(|(_, d)| one(d))
            .or_else(|| delims.iter().find_map(|(_, d)| one(d)));
        let named: std::collections::HashSet<&str> =
            delims.iter().map(|(path, _)| path.as_str()).collect();
        let unnamed: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, path FROM folders
                  WHERE account_id = ?1
                    AND CASE WHEN json_valid(sync_state_json)
                             THEN json_extract(sync_state_json, '$.delim') END IS NULL",
            )?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|(_, path)| !named.contains(path.as_str()))
                .map(|(id, _)| id)
                .collect()
        };
        let tx = self.conn.transaction()?;
        if let Some(d) = &account {
            for id in unnamed {
                tx.execute(
                    "UPDATE folders SET sync_state_json = json_set(
                        CASE WHEN json_valid(sync_state_json) THEN sync_state_json ELSE '{}' END,
                        '$.delim', ?2)
                      WHERE id = ?1",
                    params![id, d],
                )?;
            }
        }
        for (path, delim) in delims {
            match delim.as_deref().filter(|d| d.chars().count() == 1) {
                Some(d) => tx.execute(
                    "UPDATE folders SET sync_state_json = json_set(
                        CASE WHEN json_valid(sync_state_json) THEN sync_state_json ELSE '{}' END,
                        '$.delim', ?3)
                      WHERE account_id = ?1 AND path = ?2",
                    params![account_id, path, d],
                )?,
                None => tx.execute(
                    "UPDATE folders SET sync_state_json = json_remove(sync_state_json, '$.delim')
                      WHERE account_id = ?1 AND path = ?2 AND json_valid(sync_state_json)",
                    params![account_id, path],
                )?,
            };
        }
        tx.commit()?;
        Ok(())
    }

    /// The separators this folder's children are named with. See
    /// `child_separators`.
    pub(crate) fn folder_separators(&self, folder_id: i64) -> Result<Vec<char>> {
        let delim: Option<String> = self
            .conn
            .query_row(
                "SELECT CASE WHEN json_valid(sync_state_json)
                             THEN json_extract(sync_state_json, '$.delim') END
                   FROM folders WHERE id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(child_separators(delim.as_deref()))
    }

    /// Records which of the account's folders the server flags `\All`, on a
    /// server where that mailbox is not the archive — everywhere but Gmail,
    /// where All Mail is. Elsewhere it is a virtual view of every message the
    /// account holds (Dovecot's `virtual/All`, Proton Bridge's All Mail), and
    /// it still takes the archive role from the survey; synced as the archive,
    /// it downloaded the whole account a second time and listed every message
    /// outside the inbox under Archive. The flag is what keeps it out of sync.
    /// `paths` is the whole set: a folder no longer in it loses the flag.
    pub fn set_all_mail_folders(&self, account_id: i64, paths: &[String]) -> Result<()> {
        let rows: Vec<(i64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, path FROM folders WHERE account_id = ?1")?;
            let rows = stmt.query_map(params![account_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, path) in rows {
            let want = paths.contains(&path);
            if want == self.folder_flag(id, ALL_MAIL)? {
                continue;
            }
            if want {
                self.conn.execute(
                    "UPDATE folders SET sync_state_json = json_set(
                        CASE WHEN json_valid(sync_state_json) THEN sync_state_json ELSE '{}' END,
                        '$.all_mail', json('true'))
                      WHERE id = ?1",
                    params![id],
                )?;
            } else {
                self.conn.execute(
                    "UPDATE folders SET sync_state_json = json_remove(sync_state_json, '$.all_mail')
                      WHERE id = ?1 AND json_valid(sync_state_json)",
                    params![id],
                )?;
            }
        }
        Ok(())
    }

    /// Whether the server flags this folder `\All` and it is not the archive.
    pub fn folder_is_all_mail(&self, folder_id: i64) -> Result<bool> {
        self.folder_flag(folder_id, ALL_MAIL)
    }

    fn folder_flag(&self, folder_id: i64, key: &str) -> Result<bool> {
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
            .and_then(|v| v.get(key).and_then(|m| m.as_bool()))
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
