-- Petrel store schema v1.
-- Shape follows the storage design: messages are logical records; presence in a
-- folder is a separate placement relation; searchable text lives in fts_content
-- (the external-content source for both FTS indexes), written in the same
-- transaction as the message row so the index can never drift from the store.

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE accounts (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL,
    email         TEXT NOT NULL,
    display_name  TEXT,
    color         TEXT,
    settings_json TEXT NOT NULL DEFAULT '{}'
) STRICT;

CREATE TABLE folders (
    id              INTEGER PRIMARY KEY,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    role            TEXT,
    name            TEXT NOT NULL,
    path            TEXT NOT NULL,
    uidvalidity     INTEGER,
    sync_state_json TEXT NOT NULL DEFAULT '{}'
) STRICT;

CREATE TABLE blobs (
    hash TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('raw', 'assembled', 'generated')),
    size INTEGER NOT NULL
) STRICT;

CREATE TABLE messages (
    id             INTEGER PRIMARY KEY,
    account_id     INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    blob_hash      TEXT REFERENCES blobs(hash),
    blob_kind      TEXT,
    thread_id      INTEGER,
    date_ms        INTEGER NOT NULL,
    from_addr      TEXT,
    from_display   TEXT,
    subject        TEXT,
    snippet        TEXT,
    flags          INTEGER NOT NULL DEFAULT 0,
    size           INTEGER,
    message_id_hdr TEXT,
    body_state     TEXT NOT NULL DEFAULT 'full',
    doc_json       TEXT NOT NULL DEFAULT '{}'
) STRICT;
CREATE INDEX idx_messages_account_date ON messages(account_id, date_ms DESC);
CREATE INDEX idx_messages_msgid ON messages(account_id, message_id_hdr);

CREATE TABLE placements (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    folder_id   INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    uid         INTEGER,
    uidvalidity INTEGER,
    modseq      INTEGER,
    PRIMARY KEY (message_id, folder_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_placements_folder_uid ON placements(folder_id, uidvalidity, uid);

CREATE TABLE message_addresses (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    role       TEXT NOT NULL,
    addr_norm  TEXT NOT NULL,
    display    TEXT
) STRICT;
CREATE INDEX idx_addr_lookup ON message_addresses(addr_norm, role);
CREATE INDEX idx_addr_msg ON message_addresses(message_id);

CREATE TABLE actions (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state        TEXT NOT NULL DEFAULT 'queued',
    attempts     INTEGER NOT NULL DEFAULT 0,
    created_ms   INTEGER NOT NULL
) STRICT;

-- Search layer -------------------------------------------------------------

CREATE TABLE fts_content (
    message_id       INTEGER PRIMARY KEY,
    subject          TEXT NOT NULL DEFAULT '',
    body_text        TEXT NOT NULL DEFAULT '',
    addrs            TEXT NOT NULL DEFAULT '',
    attachment_names TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE VIRTUAL TABLE fts_messages USING fts5(
    subject, body_text, addrs, attachment_names,
    content='fts_content', content_rowid='message_id',
    tokenize="unicode61 remove_diacritics 2",
    prefix='2 3'
);

CREATE VIRTUAL TABLE fts_trigram USING fts5(
    subject, body_text,
    content='fts_content', content_rowid='message_id',
    tokenize="trigram"
);

CREATE TRIGGER fts_content_ai AFTER INSERT ON fts_content BEGIN
    INSERT INTO fts_messages(rowid, subject, body_text, addrs, attachment_names)
        VALUES (new.message_id, new.subject, new.body_text, new.addrs, new.attachment_names);
    INSERT INTO fts_trigram(rowid, subject, body_text)
        VALUES (new.message_id, new.subject, new.body_text);
END;

CREATE TRIGGER fts_content_ad AFTER DELETE ON fts_content BEGIN
    INSERT INTO fts_messages(fts_messages, rowid, subject, body_text, addrs, attachment_names)
        VALUES ('delete', old.message_id, old.subject, old.body_text, old.addrs, old.attachment_names);
    INSERT INTO fts_trigram(fts_trigram, rowid, subject, body_text)
        VALUES ('delete', old.message_id, old.subject, old.body_text);
END;

CREATE TRIGGER fts_content_au AFTER UPDATE ON fts_content BEGIN
    INSERT INTO fts_messages(fts_messages, rowid, subject, body_text, addrs, attachment_names)
        VALUES ('delete', old.message_id, old.subject, old.body_text, old.addrs, old.attachment_names);
    INSERT INTO fts_trigram(fts_trigram, rowid, subject, body_text)
        VALUES ('delete', old.message_id, old.subject, old.body_text);
    INSERT INTO fts_messages(rowid, subject, body_text, addrs, attachment_names)
        VALUES (new.message_id, new.subject, new.body_text, new.addrs, new.attachment_names);
    INSERT INTO fts_trigram(rowid, subject, body_text)
        VALUES (new.message_id, new.subject, new.body_text);
END;
