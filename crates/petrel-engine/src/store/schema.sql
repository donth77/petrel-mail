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
    settings_json TEXT NOT NULL DEFAULT '{}',
    -- Retention mode (Q24). 0 = mirror the server: content deleted upstream is
    -- removed here too, after a recoverable grace period. 1 = local archive:
    -- server deletions never remove local content, so the archive outlives
    -- account suspension, closure, or the provider itself.
    local_archive INTEGER NOT NULL DEFAULT 0
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
    -- Reply/forward prefixes stripped; the key two messages in one
    -- conversation should agree on when references are missing.
    subject_norm   TEXT,
    snippet        TEXT,
    flags          INTEGER NOT NULL DEFAULT 0,
    size           INTEGER,
    message_id_hdr TEXT,
    has_attachments INTEGER NOT NULL DEFAULT 0,
    -- Soft delete (Q24): set when the message leaves the server. The row and
    -- its blob survive the grace period so the deletion is recoverable; GC
    -- purges both afterwards. NULL means live.
    deleted_at_ms  INTEGER,
    body_state     TEXT NOT NULL DEFAULT 'full',
    doc_json       TEXT NOT NULL DEFAULT '{}'
) STRICT;
CREATE INDEX idx_messages_account_date ON messages(account_id, date_ms DESC);
CREATE INDEX idx_messages_deleted ON messages(deleted_at_ms) WHERE deleted_at_ms IS NOT NULL;
CREATE INDEX idx_messages_msgid ON messages(account_id, message_id_hdr);
CREATE INDEX idx_messages_thread ON messages(thread_id, date_ms DESC);
CREATE INDEX idx_messages_subject_norm ON messages(account_id, subject_norm);

-- The reply graph: one row per Message-ID this message references. Threading
-- unions over these, so a message arriving late can join two chains that were
-- separate — normal when the middle of a conversation syncs after its ends.
CREATE TABLE message_refs (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    ref_msgid  TEXT NOT NULL,
    PRIMARY KEY (message_id, ref_msgid)
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_refs_msgid ON message_refs(ref_msgid);

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

-- Attachment metadata. The bytes live inside the message's raw blob; this
-- records where to find them and what to show before anything is decoded.
CREATE TABLE attachments (
    id         INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    part_id    INTEGER NOT NULL,
    filename   TEXT,
    mime       TEXT,
    size       INTEGER,
    blob_hash  TEXT REFERENCES blobs(hash)   -- set only when stored separately
) STRICT;
CREATE INDEX idx_attachments_message ON attachments(message_id);

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

-- CJK index: one token per character, so 1- and 2-character queries match
-- (07 §5.3). The built-in trigram tokenizer cannot do this — by design it
-- matches nothing shorter than 3 characters, which silently hides the many
-- everyday two-character words in Chinese, Japanese and Korean.
--
-- Deliberately NOT external-content: the indexed text is a transform of the
-- original (CJK runs space-separated by petrel_cjk), so pointing FTS5 at
-- fts_content would make it disagree with what was actually indexed.
--
-- Contentless instead, so no second copy of the text is stored — snippets come
-- from fts_content, which already holds the original. `contentless_delete=1`
-- (SQLite 3.43+) is what makes rows deletable without that copy. Only messages
-- containing CJK are inserted, so this stays empty for mailboxes with none.
CREATE VIRTUAL TABLE fts_cjk USING fts5(
    subject, body_text,
    content='', contentless_delete=1,
    tokenize="unicode61"
);

CREATE TRIGGER fts_content_ai AFTER INSERT ON fts_content BEGIN
    INSERT INTO fts_messages(rowid, subject, body_text, addrs, attachment_names)
        VALUES (new.message_id, new.subject, new.body_text, new.addrs, new.attachment_names);
    INSERT INTO fts_cjk(rowid, subject, body_text)
        SELECT new.message_id, petrel_cjk(new.subject), petrel_cjk(new.body_text)
        WHERE petrel_has_cjk(new.subject) OR petrel_has_cjk(new.body_text);
END;

CREATE TRIGGER fts_content_ad AFTER DELETE ON fts_content BEGIN
    INSERT INTO fts_messages(fts_messages, rowid, subject, body_text, addrs, attachment_names)
        VALUES ('delete', old.message_id, old.subject, old.body_text, old.addrs, old.attachment_names);
    DELETE FROM fts_cjk WHERE rowid = old.message_id;
END;

CREATE TRIGGER fts_content_au AFTER UPDATE ON fts_content BEGIN
    INSERT INTO fts_messages(fts_messages, rowid, subject, body_text, addrs, attachment_names)
        VALUES ('delete', old.message_id, old.subject, old.body_text, old.addrs, old.attachment_names);
    DELETE FROM fts_cjk WHERE rowid = old.message_id;
    INSERT INTO fts_messages(rowid, subject, body_text, addrs, attachment_names)
        VALUES (new.message_id, new.subject, new.body_text, new.addrs, new.attachment_names);
    INSERT INTO fts_cjk(rowid, subject, body_text)
        SELECT new.message_id, petrel_cjk(new.subject), petrel_cjk(new.body_text)
        WHERE petrel_has_cjk(new.subject) OR petrel_has_cjk(new.body_text);
END;
