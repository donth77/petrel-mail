-- What the reader answered to an invitation, so the card can say
-- "Accepted" instead of offering the same three buttons forever.
-- Recorded when the METHOD:REPLY is queued; the reply itself travels
-- through the outbox like any mail.
ALTER TABLE messages ADD COLUMN invite_response TEXT;
