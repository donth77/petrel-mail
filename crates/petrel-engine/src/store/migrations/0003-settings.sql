-- User preferences.
--
-- Deliberately separate from `meta`: that table is Petrel's own bookkeeping
-- (schema version, extractor version, demo flags) and is not the user's to
-- change. Mixing the two means a preference reset risks clearing state the
-- engine depends on, and an internal key showing up in a settings export.
--
-- Values are text; the UI owns their meaning. A settings row for a key the app
-- no longer understands is ignored rather than migrated, so rolling back a
-- release cannot corrupt preferences.

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
