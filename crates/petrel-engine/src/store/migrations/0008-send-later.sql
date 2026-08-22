-- When a draft should go.
--
-- NULL means "when you press Send". A time means it is waiting, and the Outbox
-- is exactly the set of drafts with one — which is why Outbox needs no table
-- and no queue of its own.
--
-- Like snooze, nothing schedules this. "What is due" is a comparison against
-- the clock, so a message posted while the app was shut goes out on the next
-- pass rather than being missed by a timer that never fired.
ALTER TABLE messages ADD COLUMN send_after_ms INTEGER;

CREATE INDEX idx_messages_send_after ON messages(send_after_ms)
    WHERE send_after_ms IS NOT NULL;
