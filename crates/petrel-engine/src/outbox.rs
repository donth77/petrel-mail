//! Outbox state machine and the ambiguous-outcome rule.
//!
//! The hard truth this module exists for: **no mail transport offers
//! idempotent send.** SMTP, the Gmail API, and Microsoft Graph can all accept a
//! message and then fail to tell the client so — the connection dies between
//! the server's commit and the client's acknowledgement. Retrying may duplicate;
//! not retrying may silently lose the mail.
//!
//! So Petrel does not promise "a send never duplicates". It promises: **no
//! ambiguous outcome is ever silently retried or silently dropped** — it is
//! reconciled against the server, or surfaced to the user.
//!
//! The policy here is pure and I/O-free; the caller performs the lookup.

/// What a send attempt told us — deliberately including "we don't know".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Transport confirmed acceptance (250 on the final dot, 2xx from an API).
    Accepted,
    /// Failed *before* the message could have been committed: connection
    /// refused, TLS failure, auth rejected, recipient refused at RCPT.
    FailedBeforeCommit,
    /// Failed *after* the message was fully transmitted, with no acknowledgement
    /// read back. The dangerous case.
    UnknownAfterTransmit,
    /// Server refused permanently (5xx on content/policy). No retry will help.
    RejectedPermanently,
}

/// Did the sent message turn up in the account's Sent/All Mail?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerEvidence {
    /// Found by Message-ID — it was delivered.
    Found,
    /// Searched successfully and it is definitively not there.
    Absent,
    /// Could not search (offline, folder missing, provider has no Sent copy).
    Indeterminate,
}

/// Durable state of an outbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    /// Waiting out the undo window; nothing has hit the wire.
    UndoWindow,
    /// In flight.
    Transmitting,
    /// Done.
    Sent,
    /// Retry is safe and automatic.
    RetryQueued,
    /// Permanently failed; user must edit or discard.
    FailedPermanent,
    /// **Requires a human.** We cannot prove whether it was sent. Never retried
    /// automatically, never discarded.
    NeedsAttention,
}

/// The reconciliation rule. `evidence` is only consulted when the transport
/// left us uncertain — the point is that uncertainty is resolved by *looking*,
/// not by guessing.
pub fn reconcile(outcome: AttemptOutcome, evidence: ServerEvidence) -> SendState {
    match outcome {
        AttemptOutcome::Accepted => SendState::Sent,
        AttemptOutcome::RejectedPermanently => SendState::FailedPermanent,
        // Nothing was committed, so retrying cannot duplicate.
        AttemptOutcome::FailedBeforeCommit => SendState::RetryQueued,
        AttemptOutcome::UnknownAfterTransmit => match evidence {
            ServerEvidence::Found => SendState::Sent,
            ServerEvidence::Absent => SendState::RetryQueued,
            ServerEvidence::Indeterminate => SendState::NeedsAttention,
        },
    }
}

/// Whether the engine may retry this state without asking the user.
pub fn may_retry_automatically(state: SendState) -> bool {
    matches!(state, SendState::RetryQueued)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_is_sent() {
        assert_eq!(
            reconcile(AttemptOutcome::Accepted, ServerEvidence::Indeterminate),
            SendState::Sent
        );
    }

    #[test]
    fn pre_commit_failures_retry_safely() {
        // Nothing reached the server, so an automatic retry cannot duplicate.
        let state = reconcile(AttemptOutcome::FailedBeforeCommit, ServerEvidence::Absent);
        assert_eq!(state, SendState::RetryQueued);
        assert!(may_retry_automatically(state));
    }

    #[test]
    fn ambiguous_but_found_on_server_is_sent_not_resent() {
        // The duplicate-send bug in one assertion: the transport said "error",
        // the server says "I have it". We must not send it again.
        let state = reconcile(AttemptOutcome::UnknownAfterTransmit, ServerEvidence::Found);
        assert_eq!(state, SendState::Sent);
        assert!(!may_retry_automatically(state));
    }

    #[test]
    fn ambiguous_and_provably_absent_retries() {
        let state = reconcile(AttemptOutcome::UnknownAfterTransmit, ServerEvidence::Absent);
        assert_eq!(state, SendState::RetryQueued);
        assert!(may_retry_automatically(state));
    }

    #[test]
    fn ambiguous_and_unverifiable_asks_the_user() {
        // The silent-loss bug in one assertion: we cannot prove either way, so
        // we neither resend nor discard — we surface it.
        let state = reconcile(
            AttemptOutcome::UnknownAfterTransmit,
            ServerEvidence::Indeterminate,
        );
        assert_eq!(state, SendState::NeedsAttention);
        assert!(!may_retry_automatically(state));
    }

    #[test]
    fn nothing_is_ever_silently_dropped_or_silently_resent() {
        use AttemptOutcome::*;
        use ServerEvidence::*;
        for outcome in [
            Accepted,
            FailedBeforeCommit,
            UnknownAfterTransmit,
            RejectedPermanently,
        ] {
            for evidence in [Found, Absent, Indeterminate] {
                let state = reconcile(outcome, evidence);
                // Automatic retry is permitted only where duplication is
                // impossible: we know the server does not have the message.
                if may_retry_automatically(state) {
                    assert!(
                        outcome == FailedBeforeCommit
                            || (outcome == UnknownAfterTransmit && evidence == Absent),
                        "auto-retry allowed for {outcome:?}/{evidence:?} — could duplicate"
                    );
                }
                // And no path silently forgets a message.
                assert!(matches!(
                    state,
                    SendState::Sent
                        | SendState::RetryQueued
                        | SendState::FailedPermanent
                        | SendState::NeedsAttention
                ));
            }
        }
    }
}
