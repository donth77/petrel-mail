//! Triage actions: the writing half of the client.
//!
//! Every action is applied locally first and queued for the server second. That
//! ordering is the product decision, not an optimisation — archiving is the most
//! repeated gesture in a mail client, and a client that waits on a round trip
//! before the row leaves the list feels broken even when it is working.
//!
//! The consequence is that local state can be ahead of the server, so each queued
//! action carries **the state it replaced**. That is what makes undo exact rather
//! than approximate: restoring "what it was" beats inferring an inverse, which
//! goes wrong the moment two actions touch the same message.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Archive,
    Trash,
    Spam,
    Star,
    Unstar,
    MarkRead,
    MarkUnread,
    /// Move to a named folder, rather than one of the three roles above. The
    /// destination rides alongside as a target id: keeping the kind a plain
    /// string keeps one wire format for every action, and the store rejects a
    /// move that arrives without one.
    Move,
    Tag,
    Untag,
    /// Put aside until an instant, carried as the target. Local only: IMAP has
    /// nowhere to record it, so it never reaches a server and never drains.
    Snooze,
    Unsnooze,
    /// Gone. Not to the trash — out of it.
    ///
    /// The one action with no inverse. Everything else here is queued *with*
    /// the state it replaced so it can be put back; this one ends with an
    /// EXPUNGE, and no amount of local bookkeeping can un-expunge a message.
    /// So it is confirmed before it happens rather than offered as undo after,
    /// and `is_undoable` is what keeps the rest of the system honest about that.
    DeleteForever,
}

impl ActionKind {
    /// The folder a message lands in, if this action moves it at all.
    pub fn destination_role(self) -> Option<&'static str> {
        match self {
            ActionKind::Archive => Some("archive"),
            ActionKind::Trash => Some("trash"),
            ActionKind::Spam => Some("spam"),
            _ => None,
        }
    }

    /// Whether this action is meaningless without a target id — a folder for a
    /// move, a tag for tagging. Checked in the store, so an action can never be
    /// queued in a state that cannot be applied or undone.
    pub fn needs_target(self) -> bool {
        matches!(
            self,
            ActionKind::Move | ActionKind::Tag | ActionKind::Untag | ActionKind::Snooze
        )
    }

    /// Whether this action exists only in Petrel. Local actions are recorded so
    /// they can be undone, but never queued for delivery — leaving them
    /// 'queued' would strand them in the drain forever and block resync from
    /// ever trusting the server about those messages again.
    pub fn is_local_only(self) -> bool {
        matches!(self, ActionKind::Snooze | ActionKind::Unsnooze)
    }

    /// Whether the user can take this back.
    ///
    /// Only one action cannot be, and it is worth a method rather than a check
    /// at each call site: an undo offered for a permanent delete would be a
    /// button that lies, and the lie would only be discovered by someone
    /// pressing it to recover something they wanted.
    pub fn is_undoable(self) -> bool {
        !matches!(self, ActionKind::DeleteForever)
    }

    /// What the user is told after it happens. Past tense, because it already has.
    pub fn past_tense(self) -> &'static str {
        match self {
            ActionKind::Archive => "Archived",
            ActionKind::Trash => "Moved to Trash",
            ActionKind::Spam => "Reported as spam",
            ActionKind::Star => "Starred",
            ActionKind::Unstar => "Unstarred",
            ActionKind::MarkRead => "Marked read",
            ActionKind::MarkUnread => "Marked unread",
            ActionKind::Move => "Moved",
            ActionKind::Tag => "Tagged",
            ActionKind::Untag => "Untagged",
            ActionKind::Snooze => "Snoozed",
            ActionKind::Unsnooze => "Back in the inbox",
            ActionKind::DeleteForever => "Deleted",
        }
    }
}

/// How a provider models "which folder is this message in".
///
/// Classic IMAP says exactly one: a message lives in a folder, and moving it
/// removes it from where it was. Gmail presents labels as folders, so a message
/// is legitimately in INBOX *and* Work *and* Contracts at the same time, and
/// archiving means removing one label rather than clearing them all.
///
/// This is the difference the store has to model, because triage is applied
/// locally before the server ever sees it — so the local prediction has to be
/// the same shape as what the server will do. Treating Gmail as exclusive
/// silently strips every user label the first time somebody archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementPolicy {
    /// One folder per message; any move replaces what was there.
    Exclusive,
    /// Folders are labels; a message can carry several.
    Labels,
}

impl PlacementPolicy {
    /// Whether archiving should clear every placement or only the inbox one.
    ///
    /// On both providers the *user-visible* result is the same — it leaves the
    /// inbox — but on Gmail clearing the rest would also throw away labels the
    /// user applied deliberately.
    pub fn archive_clears_everything(self) -> bool {
        matches!(self, PlacementPolicy::Exclusive)
    }
}

/// One message's state before an action touched it — enough to put it back
/// exactly, including which folders it was in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorState {
    pub message_id: i64,
    pub flags: i64,
    pub folder_ids: Vec<i64>,
    /// Tags are captured for the same reason folders are: undoing a tag has to
    /// put back what was there, not remove what this action added — those differ
    /// the moment the same tag was already on the message.
    ///
    /// Defaulted because actions queued before tagging existed have no such
    /// field, and a queue that fails to deserialise is a queue that strands
    /// work the server has not seen.
    #[serde(default)]
    pub tag_ids: Vec<i64>,
    /// When this was due back, if it was snoozed. Captured like the rest so
    /// undoing a snooze restores the previous one rather than clearing it.
    #[serde(default)]
    pub snoozed_until: Option<i64>,
}

/// The queued action, and the state it replaced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPayload {
    pub kind: ActionKind,
    pub thread_id: i64,
    /// The folder or tag this action names, for the kinds that need one.
    #[serde(default)]
    pub target: Option<i64>,
    pub prior: Vec<PriorState>,
}

/// Handed back to the caller so it can undo without having to remember anything.
#[derive(Debug, Clone, Serialize)]
pub struct ActionReceipt {
    pub action_id: i64,
    pub kind: ActionKind,
    pub message_count: usize,
    /// Already past tense — the local change has happened by the time this returns.
    pub description: String,
}
