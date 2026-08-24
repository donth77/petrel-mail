-- Filter rules: on-arrival triage the user wrote down.
--
-- Conditions and actions ride as JSON because they are read by exactly one
-- consumer — the engine applying them — and never queried by parts. The
-- position column is the whole of the ordering story: rules run lowest
-- position first, deterministically, and renumbering is an UPDATE.
CREATE TABLE rules (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    name        TEXT NOT NULL,
    conditions_json TEXT NOT NULL DEFAULT '[]',
    actions_json    TEXT NOT NULL DEFAULT '{}'
) STRICT;
