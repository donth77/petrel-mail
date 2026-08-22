-- Senders whose remote content may load.
--
-- Remote content is blocked by default, because a single image fetch tells the
-- sender the address is live, roughly when it was read, and roughly from where.
-- That is the whole business model of a tracking pixel, and it is not something
-- to opt out of on a per-message basis forever.
--
-- But blocking everything makes most modern mail look broken, so there are two
-- ways out. This table is the deliberate one: "always show images from this
-- person", recorded once. The other needs no table at all — if the user has
-- written to the address, the sender already knows they exist, and the pixel
-- has nothing left to learn. That is a query over sent mail, not a stored
-- decision, so it stays correct as the mailbox changes.
CREATE TABLE remote_senders (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    addr_norm  TEXT NOT NULL,
    -- When it was trusted, so the settings pane can order the list by what the
    -- user did most recently rather than by an opaque rowid.
    added_ms   INTEGER NOT NULL,
    PRIMARY KEY (account_id, addr_norm)
) STRICT;
