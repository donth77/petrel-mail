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
        matches!(self, ActionKind::Move | ActionKind::Tag | ActionKind::Untag)
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
        }
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
