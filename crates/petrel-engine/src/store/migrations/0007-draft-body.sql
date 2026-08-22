-- A draft's full text.
--
-- `snippet` is a preview and is truncated; reopening a draft has to give back
-- every word that was written, so the body needs a column of its own. Only
-- drafts populate it — a received message's body lives in its blob, which is
-- the original bytes and the thing exports and verification depend on.
ALTER TABLE messages ADD COLUMN draft_body TEXT;
