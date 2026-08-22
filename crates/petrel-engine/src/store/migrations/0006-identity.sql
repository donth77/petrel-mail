-- The signature and how it is used.
--
-- On the account rather than in a separate identities table, because there is
-- exactly one verified identity per account until aliases can be checked with
-- the provider. Modelling several now would mean inventing a verification
-- state we cannot fill in, and offering to send as an address the server will
-- reject is worse than not offering it.
--
-- display_name already exists on accounts; this adds what goes under it.
ALTER TABLE accounts ADD COLUMN signature TEXT NOT NULL DEFAULT '';

-- Whether the signature is added to replies as well as new messages. Off by
-- default: a signature repeated down a long thread is the thing people
-- complain about, not the thing they miss.
ALTER TABLE accounts ADD COLUMN signature_on_reply INTEGER NOT NULL DEFAULT 0;
