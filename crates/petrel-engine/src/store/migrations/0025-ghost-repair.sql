-- Mail deleted on another device used to lose its placement and nothing
-- else. A message with no folder at all is out of every view, still answers
-- searches, still sits in its conversation, and is never collected. The
-- sweep now tombstones such a message the way deleting its folder does;
-- this catches up on the ones already stranded, in accounts that mirror
-- the server. A local archive keeps them on purpose. Drafts and outbox rows
-- are allowed to have no placement, and always were.
UPDATE messages SET deleted_at_ms = (strftime('%s','now') * 1000)
 WHERE deleted_at_ms IS NULL
   AND send_after_ms IS NULL
   AND draft_msgid IS NULL
   AND coalesce(draft_body, '') = ''
   AND coalesce(draft_html, '') = ''
   AND NOT EXISTS (SELECT 1 FROM placements p WHERE p.message_id = messages.id)
   AND account_id IN (SELECT id FROM accounts WHERE local_archive = 0);

-- The index row goes wherever the message row goes. Removing an account
-- cascades its messages away, and fts_content has no foreign key to follow
-- them; the rows left behind made every search that touched one fail. This
-- is what the explicit delete beside each DELETE FROM messages was doing by
-- hand, for the paths that had none. It fires once per index row that still
-- exists, so a delete that already cleared the index row costs nothing more.
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    DELETE FROM fts_content WHERE message_id = old.id;
END;

-- The rows already orphaned, and the index rows of tombstoned messages —
-- every tombstone path clears those, and the ghosts above never took one.
DELETE FROM fts_content
 WHERE message_id NOT IN (SELECT id FROM messages WHERE deleted_at_ms IS NULL);
