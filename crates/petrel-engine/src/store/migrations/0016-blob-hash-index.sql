-- The gc orphan sweep asks, for every blob, whether any message or
-- attachment still points at it. Unindexed, each of those NOT EXISTS
-- probes is a full scan of its table — at twenty-eight thousand blobs
-- against twenty-eight thousand messages, a minute and a half of
-- startup spent proving that nothing needed collecting. Indexed, the
-- same sweep is under a hundred milliseconds.
CREATE INDEX IF NOT EXISTS idx_messages_blob_hash
    ON messages(blob_hash);
CREATE INDEX IF NOT EXISTS idx_attachments_blob_hash
    ON attachments(blob_hash);
