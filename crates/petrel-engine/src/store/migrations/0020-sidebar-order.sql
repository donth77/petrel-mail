-- A place to remember the order somebody dragged their folders and tags into.
--
-- NULL means "never dragged", and that is the point of allowing it. Sorting
-- puts the untouched ones after the arranged ones, each group still
-- alphabetical, so arranging one folder does not silently reshuffle every
-- folder nobody has touched. A DEFAULT 0 would have made every row equal and
-- handed the tie-break to whatever the query felt like that day.
--
-- Local, deliberately. Folders come from the server and their order is not the
-- server's concept: IMAP has no notion of one, so an order pushed there has
-- nowhere to go. It travels in the settings export like every other
-- preference, and a fresh install shows the provider's own ordering until
-- somebody rearranges it.
ALTER TABLE folders ADD COLUMN sort_order INTEGER;
ALTER TABLE tags ADD COLUMN sort_order INTEGER;

-- Reading order is per account and always sorted, so the index carries the
-- sort rather than making every sidebar render pay for one.
CREATE INDEX IF NOT EXISTS idx_folders_order ON folders(account_id, sort_order);
CREATE INDEX IF NOT EXISTS idx_tags_order ON tags(account_id, sort_order);
