-- Delivery is per message, not per action. An archive of a three-message
-- conversation is three server operations, and marking the action sent on
-- the first success threw the other two away: they were never retried, and
-- the next sync walked half the conversation back into the inbox. Each row
-- now records its own outcome, and the action's state follows the last row.
ALTER TABLE action_messages ADD COLUMN delivered_ms INTEGER;
ALTER TABLE action_messages ADD COLUMN dropped_ms INTEGER;
