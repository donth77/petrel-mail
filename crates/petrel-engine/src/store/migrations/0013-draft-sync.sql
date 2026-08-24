-- A draft's identity on the server.
--
-- Pushing a draft to the server's Drafts folder needs two facts the draft did
-- not keep: the Message-ID it travels under, which stays stable across every
-- autosave so the server copy is an edit and not a sibling — and so the copy,
-- fetched back by ordinary folder sync, dedupes onto this very row instead of
-- appearing beside it; and the UID of the copy currently on the server, which
-- is what replacing it deletes. NULL for a draft never pushed, and for every
-- message that is not a draft at all.
ALTER TABLE messages ADD COLUMN draft_msgid TEXT;
ALTER TABLE messages ADD COLUMN draft_server_uid INTEGER;
