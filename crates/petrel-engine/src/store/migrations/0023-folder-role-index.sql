-- Inbox and folder counts now start from placements rather than from every
-- message with a correlated EXISTS. That plan looks up folders by
-- (account_id, role); without this index SQLite scans the folder list on
-- every recount — cheap at a dozen folders, not the thing to discover at
-- a hundred labels.
CREATE INDEX IF NOT EXISTS idx_folders_account_role
    ON folders(account_id, role);

-- Starred is a handful of rows in a mailbox of hundreds of thousands. Without
-- a partial index, counting them walks every live message to test the flag.
CREATE INDEX IF NOT EXISTS idx_messages_flagged
    ON messages(account_id, coalesce(thread_id, -id))
    WHERE deleted_at_ms IS NULL AND flags & 4 != 0;
