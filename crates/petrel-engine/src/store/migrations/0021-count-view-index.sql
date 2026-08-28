-- Counting the conversations in a view groups every matching message by
-- coalesce(thread_id, -id). Nothing indexed that expression *per account*, so
-- SQLite read the account's messages and then sorted them into a temporary
-- b-tree to find the distinct keys — the sort being most of the 120ms.
--
-- idx_messages_thread_key already indexes the expression, but it leads with
-- the key rather than the account, so it cannot serve "this account's messages,
-- in key order". Leading with account_id can.
--
-- Partial on the live rows: deleted mail is excluded by every caller, and
-- leaving tombstones out keeps the index the size of the mailbox somebody
-- actually has rather than the size of everything it has ever held.
CREATE INDEX IF NOT EXISTS idx_messages_account_thread
    ON messages(account_id, coalesce(thread_id, -id))
    WHERE deleted_at_ms IS NULL;
