-- Tags: IMAP keywords / Gmail labels / Graph categories.
--
-- Names sync (they are the provider's own concept and travel with the account);
-- colours are local, because no provider has a portable notion of one. See
-- docs 07 §7.0 for why tags, folders and saved searches are kept distinct.

CREATE TABLE tags (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    colour     TEXT,
    UNIQUE(account_id, name)
) STRICT;

CREATE TABLE message_tags (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, tag_id)
) STRICT;

CREATE INDEX idx_message_tags_tag ON message_tags(tag_id);
