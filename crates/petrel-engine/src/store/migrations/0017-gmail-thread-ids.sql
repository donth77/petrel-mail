-- Gmail's own conversation ids. JWZ threading works from References
-- headers, and mail that arrives without them — most notification and
-- newsletter mail — threads alone, so a Gmail inbox counted ~655
-- conversations where Gmail's UI said ~271. X-GM-THRID is Gmail saying
-- which conversation each message belongs to; where it is known, it is
-- authoritative and local threading defers to it.
ALTER TABLE messages ADD COLUMN gm_thrid INTEGER;
CREATE INDEX IF NOT EXISTS idx_messages_gm_thrid
    ON messages(account_id, gm_thrid);
