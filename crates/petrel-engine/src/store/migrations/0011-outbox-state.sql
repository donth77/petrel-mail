-- The outbox as a state machine rather than a flag.
--
-- `send_after_ms` alone said only "this is waiting". A message that the server
-- refused, one waiting for a connection, and one whose fate is genuinely
-- unknown all looked identical — and the last of those is the one that must
-- never be retried on its own, because retrying it can send it twice.
--
-- `send_state` holds petrel_engine::outbox::SendState by name.
-- `send_error` is the server's words, for the row to show.
-- `send_attempts` and `send_next_ms` drive the retry ladder.
-- `send_message_id` is the Message-ID the attempt went out under, which is
-- what a Sent-folder search looks for when the outcome was ambiguous.
ALTER TABLE messages ADD COLUMN send_state TEXT;
ALTER TABLE messages ADD COLUMN send_error TEXT;
ALTER TABLE messages ADD COLUMN send_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN send_next_ms INTEGER;
ALTER TABLE messages ADD COLUMN send_message_id TEXT;
