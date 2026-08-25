-- The conversation list joins and groups on coalesce(thread_id, -id) — an
-- expression, which no plain column index can serve. At six thousand
-- messages the join degenerated into a scan of every message for every
-- thread: ten seconds to list an inbox. An index on the expression itself
-- (with the date, which the join also names) makes it a lookup.
CREATE INDEX IF NOT EXISTS idx_messages_thread_key
    ON messages(coalesce(thread_id, -id), date_ms);
