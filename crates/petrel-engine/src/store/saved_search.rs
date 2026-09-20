//! Saved searches: a question, pinned under a name.
//!
//! The one part of the pinned-view model ([06 §2](../../../docs/06-ux-design.md))
//! that the field could not do for itself. A view is a query; a saved search is
//! a query with a name and a place in the sidebar, and nothing else. Membership
//! is never stored, so there is no state here to fall out of step with the
//! mailbox — asking is the only thing a saved search does.
//!
//! No count and no schedule. A badge would mean running every saved search
//! whenever the mailbox changed, and counting is paid by how much a query
//! matches: `is:unread` counts in 73ms at a hundred thousand messages to say
//! what the Inbox row says already. A saved search is asked when it is opened.
//!
//! The query is kept as typed. Parsing it here and writing it back would be
//! lossless (the grammar round-trips, and a property test says so), but it
//! would also quietly rewrite what somebody typed into its canonical spelling
//! the first time they opened it, and the field is meant to give back what it
//! was given.

use super::{Result, Store};
use crate::search_query::{Renamed, renamed};
use rusqlite::{OptionalExtension, params};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedSearch {
    pub id: i64,
    /// What the sidebar calls it. Never the query text: a list of searches
    /// named after their own queries is a list nobody reads.
    pub name: String,
    /// What goes back into the search field, character for character.
    pub query: String,
    pub position: i64,
}

impl Store {
    /// This account's saved searches, in sidebar order.
    pub fn saved_searches(&self, account_id: i64) -> Result<Vec<SavedSearch>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, query, position
             FROM saved_searches WHERE account_id = ?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map(params![account_id], |r| {
            Ok(SavedSearch {
                id: r.get(0)?,
                name: r.get(1)?,
                query: r.get(2)?,
                position: r.get(3)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// One of them, whatever account it belongs to. The sidebar has the id and
    /// the account is not in the click.
    pub fn saved_search(&self, id: i64) -> Result<Option<SavedSearch>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name, query, position FROM saved_searches WHERE id = ?1")?;
        stmt.query_row(params![id], |r| {
            Ok(SavedSearch {
                id: r.get(0)?,
                name: r.get(1)?,
                query: r.get(2)?,
                position: r.get(3)?,
            })
        })
        .optional()
        .map_err(Into::into)
    }

    /// Creates one, at the end of the order, and says which it is.
    ///
    /// The caller has already run the query — that is the only way to reach
    /// this — so nothing is validated here beyond the name being a name.
    pub fn create_saved_search(&mut self, account_id: i64, name: &str, query: &str) -> Result<i64> {
        let position: i64 = self.conn.query_row(
            "SELECT coalesce(max(position), -1) + 1 FROM saved_searches WHERE account_id = ?1",
            params![account_id],
            |r| r.get(0),
        )?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO saved_searches(account_id, position, name, query, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account_id, position, name.trim(), query, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Changes whichever parts were given and leaves the rest as they were.
    ///
    /// Two separate things happen to a saved search — renaming it, and updating
    /// it to the query now in the field — and they happen from two different
    /// places in the window. One call each would be two round trips to say
    /// "this one, slightly different".
    pub fn update_saved_search(
        &mut self,
        id: i64,
        name: Option<&str>,
        query: Option<&str>,
    ) -> Result<()> {
        if let Some(name) = name {
            self.conn.execute(
                "UPDATE saved_searches SET name = ?2 WHERE id = ?1",
                params![id, name.trim()],
            )?;
        }
        if let Some(query) = query {
            self.conn.execute(
                "UPDATE saved_searches SET query = ?2 WHERE id = ?1",
                params![id, query],
            )?;
        }
        Ok(())
    }

    pub fn delete_saved_search(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM saved_searches WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Rewrites this account's saved searches after something they can name was
    /// renamed, and says how many changed.
    ///
    /// A query holds names rather than ids, so a rename leaves it looking for
    /// something that is not there any more — silently, because a search that
    /// finds nothing looks exactly like a search whose answer is nothing. The
    /// rewrite is text in and text out, which is safe only because the grammar
    /// round-trips: what the query said about anything else comes back spelled
    /// as it was typed.
    ///
    /// Scoped to the account, because tags and folders are. Another account's
    /// `tag:urgent` is another tag.
    pub fn rename_in_saved_searches(&self, account_id: i64, what: &Renamed) -> Result<usize> {
        let mut changed = 0;
        for search in self.saved_searches(account_id)? {
            let Some(next) = renamed(&search.query, what) else {
                continue;
            };
            if next == search.query {
                continue;
            }
            self.conn.execute(
                "UPDATE saved_searches SET query = ?2 WHERE id = ?1",
                params![search.id, next],
            )?;
            changed += 1;
        }
        Ok(changed)
    }

    /// The rail's order, as dragged: the ids in their new order, numbered from
    /// zero.
    ///
    /// The same rule `set_order` follows for folders and tags, and for the same
    /// reason — renumbering the whole set is safe to repeat, leaves no gaps to
    /// run out of, and cannot end with two rows claiming one position. Rows not
    /// in the list keep what they had, so one account's order is never
    /// disturbed by a drag in another's.
    pub fn reorder_saved_searches(&mut self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE saved_searches SET position = ?1 WHERE id = ?2")?;
            for (position, id) in ids.iter().enumerate() {
                stmt.execute(params![position as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Swaps one a step up or down its account's order.
    ///
    /// The same shape as `move_rule`, and for the same reason: the order is
    /// the user's and nothing else expresses it, so moving is a swap rather
    /// than a renumbering of everything.
    pub fn move_saved_search(&mut self, id: i64, up: bool) -> Result<()> {
        let Some((account, position)) = self
            .conn
            .query_row(
                "SELECT account_id, position FROM saved_searches WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?
        else {
            return Ok(());
        };
        let neighbour: Option<(i64, i64)> = self
            .conn
            .query_row(
                &format!(
                    "SELECT id, position FROM saved_searches
                     WHERE account_id = ?1 AND position {} ?2
                     ORDER BY position {} LIMIT 1",
                    if up { "<" } else { ">" },
                    if up { "DESC" } else { "ASC" },
                ),
                params![account, position],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((other_id, other_position)) = neighbour {
            let tx = self.conn.transaction()?;
            tx.execute(
                "UPDATE saved_searches SET position = ?2 WHERE id = ?1",
                params![id, other_position],
            )?;
            tx.execute(
                "UPDATE saved_searches SET position = ?2 WHERE id = ?1",
                params![other_id, position],
            )?;
            tx.commit()?;
        }
        Ok(())
    }
}
