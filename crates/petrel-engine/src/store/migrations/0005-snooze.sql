-- Snooze: when a conversation should come back.
--
-- A column rather than a table because it is one nullable instant per message
-- and every query that cares needs it inline.
--
-- Note what is *not* here: a scheduler. "Show me the inbox" already means "mail
-- that is not snoozed past now", so a snoozed conversation reappears because
-- the clock moved, not because a job woke up and moved it. There is no timer to
-- miss, nothing to catch up on after the app was closed for a week, and no way
-- for the queue and the mailbox to disagree.
--
-- Local by design: IMAP has nowhere to put this, so the message stays in the
-- inbox in every other client. The picker says so rather than letting people
-- discover it from their phone.
ALTER TABLE messages ADD COLUMN snoozed_until_ms INTEGER;

CREATE INDEX idx_messages_snoozed ON messages(snoozed_until_ms)
    WHERE snoozed_until_ms IS NOT NULL;
