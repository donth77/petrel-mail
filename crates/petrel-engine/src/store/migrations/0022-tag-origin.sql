-- Where a tag came from, so an abandoned one can be cleared up without
-- touching one somebody made.
--
-- Petrel promotes an IMAP keyword it finds on a message into a sidebar tag.
-- That is the right behaviour: a tag applied in another client should show up
-- here. What was missing is the other half. When the last message carrying a
-- keyword goes away — deleted, moved, or untagged elsewhere — untag_message
-- removes the link and the tag row stays, so the sidebar keeps an entry that
-- labels nothing and that nobody remembers creating. A live account grew a
-- "Followup" tag with zero messages exactly this way.
--
-- The rows cannot be told apart after the fact, so this only records it from
-- now on. Existing tags default to 'user', which is the answer that never
-- deletes anything somebody meant to keep: an empty tag of your own making is
-- yours to remove. An orphan from before this migration has to be deleted by
-- hand, once.
ALTER TABLE tags ADD COLUMN origin TEXT NOT NULL DEFAULT 'user';

-- The cleanup asks "server-made tags on this account with no messages", so it
-- is worth an index rather than a scan of every tag on every sync.
CREATE INDEX IF NOT EXISTS idx_tags_origin ON tags(account_id, origin);
