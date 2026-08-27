-- When a message arrived in the bin, which is not when it was sent.
--
-- Expiry has to mean "thirty days in the Trash", not "thirty days old":
-- filing a two-year-old receipt would otherwise delete it immediately.
-- Stamped when a trash placement first appears — whether from triage here
-- or from another client, which is why it is maintained on sync rather
-- than only where the user clicks — and cleared if the message leaves.
ALTER TABLE messages ADD COLUMN trashed_at_ms INTEGER;
CREATE INDEX IF NOT EXISTS idx_messages_trashed_at
    ON messages(account_id, trashed_at_ms) WHERE trashed_at_ms IS NOT NULL;
