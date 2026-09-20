-- Saved searches: a question, pinned under a name (docs 22, 07 §7.0).
--
-- The query rides as the text that was typed, in the search grammar, because
-- the grammar *is* the criteria language: anything the field accepts is
-- savable, brackets and OR included, and there is no second place a search can
-- live. Stored as text rather than as a parsed shape so that what comes back
-- into the field is what went in, character for character.
--
-- Nothing about membership is stored. A saved search is re-asked every time,
-- which is the whole difference between it and a folder.
--
-- Per account, like rules and folders: a search is scoped to the account on
-- screen (`store/search.rs`), so a saved search belongs to one and leaves with
-- it.
--
-- No badge. A count for the sidebar would mean running the query on a
-- schedule, and counting is paid by how much a query matches rather than by
-- how complicated it is: at a hundred thousand messages `is:unread` counts in
-- 73ms — the widest query there is, telling you what the Inbox row already
-- says — while a Boolean query matching nothing counts in 6ms. Owner decision,
-- 2026-09-20: not worth running them for a number. A saved search is asked
-- when it is opened, which is what "a question, re-asked each time" meant.
--
-- IF NOT EXISTS because a migration gets replayed in tests: `hard_cases.rs`
-- winds user_version back to put an older migration under test, and everything
-- after it runs again on a store that already has its tables. A step that
-- cannot be repeated makes every such test the next migration's problem.
CREATE TABLE IF NOT EXISTS saved_searches (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    name        TEXT NOT NULL,
    query       TEXT NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;
