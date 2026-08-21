//! Retention policy — what happens locally when mail leaves the server.
//!
//! The decision (Q24) has three layers, and the binding rule is that the active
//! mode is always **stated, never emergent**. The failure this exists to
//! prevent is a user believing something is deleted when it isn't, or archived
//! when it isn't.
//!
//! 1. **Mirror (default).** A message gone from the server disappears from
//!    views here too, because that is what "delete" means to everyone.
//! 2. **Grace period.** The row and its bytes survive on disk, recoverable, for
//!    [`DEFAULT_GRACE_DAYS`] — covering the fat-finger and the wipe by someone
//!    who got into the account — and are then purged for real.
//! 3. **Local archive (opt-in, per account).** Server deletions stop removing
//!    content, so the archive outlives suspension, closure, and the provider.

/// How long deleted mail stays recoverable before GC destroys it. Long enough
/// to notice a mistake; short enough that "deleted" means deleted.
pub const DEFAULT_GRACE_DAYS: i64 = 30;

pub const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// Per-account retention mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionMode {
    /// Follow the server: upstream deletions remove local content (after grace).
    Mirror,
    /// Keep everything ever synced, regardless of the server.
    LocalArchive,
}

impl RetentionMode {
    pub fn from_flag(local_archive: bool) -> Self {
        if local_archive {
            RetentionMode::LocalArchive
        } else {
            RetentionMode::Mirror
        }
    }

    /// Whether a message absent from the server should be removed from view.
    pub fn removes_on_server_delete(self) -> bool {
        matches!(self, RetentionMode::Mirror)
    }

    /// One line of honest UI copy. Every surface that can destroy mail shows
    /// this, so the mode is never something the user has to infer.
    pub fn describe(self) -> &'static str {
        match self {
            RetentionMode::Mirror => {
                "Mirroring the server: mail deleted elsewhere is removed here too, \
                 and stays recoverable for 30 days."
            }
            RetentionMode::LocalArchive => {
                "Local archive: mail stays on this device even if it is deleted \
                 from the server."
            }
        }
    }
}

/// Whether a soft-deleted message is past its grace period.
pub fn is_purgeable(deleted_at_ms: i64, now_ms: i64, grace_days: i64) -> bool {
    now_ms.saturating_sub(deleted_at_ms) >= grace_days.saturating_mul(MS_PER_DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_removes_archive_does_not() {
        assert!(RetentionMode::Mirror.removes_on_server_delete());
        assert!(!RetentionMode::LocalArchive.removes_on_server_delete());
        assert_eq!(RetentionMode::from_flag(false), RetentionMode::Mirror);
        assert_eq!(RetentionMode::from_flag(true), RetentionMode::LocalArchive);
    }

    #[test]
    fn grace_period_boundary() {
        let deleted = 1_000_000_000_000;
        assert!(
            !is_purgeable(deleted, deleted, 30),
            "not purgeable immediately"
        );
        assert!(
            !is_purgeable(deleted, deleted + 29 * MS_PER_DAY, 30),
            "still recoverable inside the window"
        );
        assert!(
            is_purgeable(deleted, deleted + 30 * MS_PER_DAY, 30),
            "purgeable once the window closes"
        );
    }

    #[test]
    fn a_backwards_clock_cannot_purge_early() {
        // Clock skew must never shorten the grace period — saturating_sub keeps
        // "deleted in the future" from reading as "long past due".
        let deleted = 2_000_000_000_000;
        let now = deleted - 60 * MS_PER_DAY;
        assert!(!is_purgeable(deleted, now, 30));
    }

    #[test]
    fn both_modes_state_themselves() {
        for mode in [RetentionMode::Mirror, RetentionMode::LocalArchive] {
            let text = mode.describe();
            assert!(!text.is_empty());
            // The copy must say what happens to the user's mail, not name a mode.
            assert!(text.contains("mail") || text.contains("Mail"));
        }
    }
}
