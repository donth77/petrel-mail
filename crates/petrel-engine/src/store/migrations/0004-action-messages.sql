-- Which messages each queued action touches.
--
-- The payload already carries this inside its JSON, but a resync has to ask
-- "does this message have work the server has not seen yet?" on every single
-- message it ingests, and that question cannot be asked of a JSON blob without
-- scanning the whole queue. This is that question as an index.
--
-- It is also what stops a resync from silently undoing local triage: the server
-- is authoritative about a message only once our pending changes to it have
-- been delivered.
CREATE TABLE action_messages (
    action_id  INTEGER NOT NULL REFERENCES actions(id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    PRIMARY KEY (action_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX idx_action_messages_message ON action_messages(message_id);
