//! Filter rules: triage the user wrote down once, applied on arrival.
//!
//! A rule is conditions over the envelope — who it is from, who it is to,
//! what it is about, which list sent it — and actions that are exactly the
//! triage verbs the app already has. Rules run in the order the user put
//! them, every enabled rule that matches, so two rules can each contribute
//! (one tags, one archives) without a hidden first-match-wins surprise.

use crate::actions::ActionKind;
use serde::{Deserialize, Serialize};

/// One thing that must be true of the message. All of a rule's conditions
/// must hold — "and", because "mail from Dana about invoices" is the rule
/// people mean; two alternatives are two rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Condition {
    /// from | to | subject | list_id
    pub field: String,
    /// Case-insensitive substring. Contains, not equals: an address is
    /// rarely typed whole, and a subject never is.
    pub contains: String,
}

/// What a matching rule does. Any subset; each maps to the ordinary triage
/// action, queued to the server like a hand-made one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actions {
    #[serde(default)]
    pub move_to: Option<i64>,
    #[serde(default)]
    pub tag: Option<i64>,
    #[serde(default)]
    pub mark_read: bool,
    #[serde(default)]
    pub skip_inbox: bool,
    /// Announce this arrival even though the rule files it away. Mail a
    /// rule moves out of the inbox never reaches the list the announcer
    /// watches, so without this a rule is also a silencer — and some
    /// filed mail is exactly the mail worth interrupting for.
    #[serde(default)]
    pub notify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rule {
    pub id: i64,
    pub position: i64,
    pub enabled: bool,
    pub name: String,
    pub conditions: Vec<Condition>,
    pub actions: Actions,
}

/// The envelope a rule can see. Everything lowercased once by the caller's
/// constructor, so matching never re-lowers per rule.
#[derive(Debug, Default)]
pub struct Envelope {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub list_id: String,
}

impl Envelope {
    pub fn new(from: &str, to: &str, subject: &str, list_id: &str) -> Self {
        Envelope {
            from: from.to_lowercase(),
            to: to.to_lowercase(),
            subject: subject.to_lowercase(),
            list_id: list_id.to_lowercase(),
        }
    }

    /// The envelope as a rule sees a real message.
    ///
    /// Here rather than at the sync loop's call site for the reason
    /// `planned_actions` is: what a condition can see is part of what a rule
    /// *means*, and it wants testing without an IMAP pass wrapped around it.
    ///
    /// Both sides carry display names. `from` always did and `to` did not, so
    /// "To contains Dana" could never match `Dana Wu <dana@example.com>` — nor
    /// anything else, ever — while the rule sat in the list looking enabled.
    /// Matching a recipient by name is what every mail client does; being
    /// unable to was this one's own quiet invention.
    ///
    /// Cc is deliberately absent. Clients keep To and Cc as separate
    /// conditions and offer "To or Cc" as a third, rather than quietly
    /// widening one of them, because a condition that says To and means more
    /// than To cannot be reasoned about.
    pub fn from_message(parsed: &petrel_mime::ParsedMessage) -> Self {
        fn named(display: Option<&str>, addr: &str) -> String {
            match display {
                Some(d) if !d.is_empty() => format!("{d} {addr}"),
                _ => addr.to_string(),
            }
        }
        Envelope::new(
            &named(
                parsed.from_display.as_deref(),
                parsed.from_addr.as_deref().unwrap_or(""),
            ),
            &parsed
                .to
                .iter()
                .map(|(display, addr)| named(display.as_deref(), addr))
                .collect::<Vec<_>>()
                .join(", "),
            parsed.subject.as_deref().unwrap_or(""),
            parsed.list_id.as_deref().unwrap_or(""),
        )
    }
}

/// Whether every condition holds. A rule with no conditions matches nothing:
/// an empty rule that matched everything would refile the whole inbox the
/// moment it was half-created.
pub fn matches(rule: &Rule, envelope: &Envelope) -> bool {
    if !rule.enabled || rule.conditions.is_empty() {
        return false;
    }
    rule.conditions.iter().all(|c| {
        let hay = match c.field.as_str() {
            "from" => &envelope.from,
            "to" => &envelope.to,
            "subject" => &envelope.subject,
            "list_id" => &envelope.list_id,
            // A field this build does not know (written by a newer one):
            // never match, rather than matching everything.
            _ => return false,
        };
        !c.contains.is_empty() && hay.contains(&c.contains.to_lowercase())
    })
}

/// The triage actions a matching rule queues, in the order they must run.
///
/// Here rather than at the call site because the ordering carries a trap that
/// only shows up once two actions touch the same message. "Skip inbox" is
/// Archive, and Archive on a folder-style account clears every placement
/// before filing in archive — so a rule that both moved a message and skipped
/// the inbox used to queue Move then Archive, and the Archive threw the move
/// away. "Move to Marketing" plus "Skip inbox" filed the mail in Archive, not
/// Marketing, and said nothing about it.
///
/// A move already takes the message out of the inbox, so the two are not
/// really separate instructions: when a destination is named, the move *is*
/// the skip.
pub fn planned_actions(a: &Actions) -> Vec<(ActionKind, Option<i64>)> {
    let mut acts = Vec::new();
    match a.move_to {
        Some(folder) => acts.push((ActionKind::Move, Some(folder))),
        // Only meaningful on its own: with nowhere to go, skipping the inbox
        // is exactly what archiving means.
        None if a.skip_inbox => acts.push((ActionKind::Archive, None)),
        None => {}
    }
    if let Some(tag) = a.tag {
        acts.push((ActionKind::Tag, Some(tag)));
    }
    if a.mark_read {
        acts.push((ActionKind::MarkRead, None));
    }
    acts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(conds: &[(&str, &str)]) -> Rule {
        Rule {
            id: 1,
            position: 0,
            enabled: true,
            name: "r".into(),
            conditions: conds
                .iter()
                .map(|(f, c)| Condition {
                    field: f.to_string(),
                    contains: c.to_string(),
                })
                .collect(),
            actions: Actions::default(),
        }
    }

    #[test]
    fn conditions_are_case_insensitive_contains_joined_by_and() {
        let env = Envelope::new(
            "Dana Wu <dana@vendorco.example>",
            "me@example.com",
            "Q3 Invoice attached",
            "billing.vendorco.example",
        );
        assert!(matches(&rule(&[("from", "VENDORCO")]), &env));
        assert!(matches(
            &rule(&[("subject", "invoice"), ("from", "dana@")]),
            &env
        ));
        assert!(!matches(
            &rule(&[("subject", "invoice"), ("from", "nobody")]),
            &env
        ));
        assert!(matches(&rule(&[("list_id", "billing.")]), &env));
        assert!(!matches(&rule(&[("to", "someone-else")]), &env));
    }

    /// The bug: "Move to Marketing" plus "Skip inbox" filed mail in Archive.
    ///
    /// Archive clears every placement on a folder-style account before filing,
    /// so queueing Move then Archive threw the move away. Proven against a
    /// real store before the fix: Marketing=0, archive=1.
    #[test]
    fn a_move_is_not_undone_by_skipping_the_inbox() {
        let both = Actions {
            move_to: Some(7),
            skip_inbox: true,
            ..Actions::default()
        };
        let planned = planned_actions(&both);
        assert_eq!(
            planned,
            vec![(ActionKind::Move, Some(7))],
            "a named destination is the skip; Archive after it discards the move"
        );

        // Skipping the inbox with nowhere to go still means archive.
        let skip_only = Actions {
            skip_inbox: true,
            ..Actions::default()
        };
        assert_eq!(
            planned_actions(&skip_only),
            vec![(ActionKind::Archive, None)]
        );
    }

    #[test]
    fn every_action_a_rule_can_carry_is_queued() {
        let all = Actions {
            move_to: Some(3),
            tag: Some(5),
            mark_read: true,
            skip_inbox: true,
            notify: true,
        };
        // notify is not a triage action: it is announced, not applied.
        assert_eq!(
            planned_actions(&all),
            vec![
                (ActionKind::Move, Some(3)),
                (ActionKind::Tag, Some(5)),
                (ActionKind::MarkRead, None),
            ]
        );
        assert!(planned_actions(&Actions::default()).is_empty());
    }

    /// The bug: a recipient condition could only ever see the address, so
    /// "To contains Dana" matched nothing at all — not this message, not any
    /// message — while the rule sat in the list looking enabled.
    ///
    /// `from` was built from the display name *and* the address; `to` threw
    /// the display names away. Nothing said so, and a rule that never fires
    /// looks exactly like a rule whose mail has not arrived yet.
    #[test]
    fn a_recipient_is_matched_by_name_as_well_as_by_address() {
        let raw = b"From: Dana Wu <dana@vendorco.example>\r\n\
                    To: Sam Okafor <sam@example.com>, billing@example.com\r\n\
                    Cc: Ada Chen <ada@example.com>\r\n\
                    Subject: Q3 Invoice attached\r\n\
                    Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
                    Message-ID: <inv1@x>\r\nMIME-Version: 1.0\r\n\
                    Content-Type: text/plain\r\n\r\nbody\r\n";
        let parsed = petrel_mime::parse_message(raw).expect("parses");
        let env = Envelope::from_message(&parsed);

        assert!(matches(&rule(&[("to", "Sam Okafor")]), &env), "by name");
        assert!(matches(&rule(&[("to", "sam@example")]), &env), "by address");
        assert!(
            matches(&rule(&[("to", "billing@")]), &env),
            "second recipient"
        );
        // The half that always worked, still working the same way.
        assert!(matches(&rule(&[("from", "Dana Wu")]), &env));
        assert!(matches(&rule(&[("from", "dana@vendorco")]), &env));
        assert!(
            !matches(&rule(&[("to", "dana")]), &env),
            "the sender is not a recipient"
        );

        // Cc is not silently folded into `to`. Every client keeps them apart
        // and offers "To or Cc" as its own condition; a To that quietly meant
        // more than To could not be reasoned about.
        assert!(!matches(&rule(&[("to", "Ada Chen")]), &env));
        assert!(!matches(&rule(&[("to", "ada@example")]), &env));
    }

    #[test]
    fn empty_disabled_and_unknown_rules_match_nothing() {
        let env = Envelope::new("a@x", "b@x", "s", "");
        assert!(!matches(&rule(&[]), &env), "no conditions is no match");
        let mut r = rule(&[("from", "a@x")]);
        r.enabled = false;
        assert!(!matches(&r, &env), "disabled is disabled");
        assert!(
            !matches(&rule(&[("headers", "x")]), &env),
            "an unknown field never matches"
        );
        assert!(
            !matches(&rule(&[("from", "")]), &env),
            "an empty needle would match the world"
        );
    }
}
