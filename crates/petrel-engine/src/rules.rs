//! Filter rules: triage the user wrote down once, applied on arrival.
//!
//! A rule is conditions over the envelope — who it is from, who it is to,
//! what it is about, which list sent it — and actions that are exactly the
//! triage verbs the app already has. Rules run in the order the user put
//! them, every enabled rule that matches, so two rules can each contribute
//! (one tags, one archives) without a hidden first-match-wins surprise.

use crate::actions::ActionKind;
use serde::{Deserialize, Serialize};

/// How a condition compares.
///
/// Text and numbers want different questions, so the set is not uniform: no
/// field takes all of these. What a field offers is `Field::ops`, and the
/// editor is built from that rather than from a fixed list, so a rule that
/// cannot be expressed also cannot be typed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Contains,
    NotContains,
    Is,
    IsNot,
    StartsWith,
    NotStartsWith,
    EndsWith,
    NotEndsWith,
    /// Size, in kilobytes as the person typed it.
    Over,
    Under,
    /// Sent date, against a `YYYY-MM-DD` the person picked.
    Before,
    After,
}

impl Op {
    /// The negations, so matching states each test once and flips it. Writing
    /// eight arms instead of four is how "does not end with" ends up meaning
    /// something subtly different from "not (ends with)".
    fn negated(self) -> bool {
        matches!(
            self,
            Op::NotContains | Op::IsNot | Op::NotStartsWith | Op::NotEndsWith
        )
    }
}

fn default_op() -> Op {
    Op::Contains
}

/// One thing that must be true of the message. All of a rule's conditions
/// must hold — "and", because "mail from Dana about invoices" is the rule
/// people mean; two alternatives are two rules.
///
/// `value` was called `contains` when substring was the only test there was,
/// and rules written then are still on disk. The alias is what lets them
/// load: an old row has no `op` and takes the default, which is the substring
/// test it was written under, so a rule somebody wrote a month ago goes on
/// meaning exactly what it meant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Condition {
    /// from | to | cc | subject | body | header | size | date
    pub field: String,
    /// Which header, when `field` is `header`. Ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default = "default_op")]
    pub op: Op,
    #[serde(alias = "contains")]
    pub value: String,
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
    pub cc: String,
    pub subject: String,
    pub list_id: String,
    /// The text of the message, as the index sees it: the plain part, or the
    /// HTML part with its tags taken off. Matching markup would mean a rule
    /// for "invoice" hitting a message whose only "invoice" was a CSS class.
    pub body: String,
    /// Whole bytes on the wire, which is the number a mail client shows and
    /// therefore the number somebody writing a rule has in mind.
    pub size: u64,
    /// When the message says it was sent. Milliseconds, as everything here.
    pub date_ms: i64,
    /// Every header, lowercased name, value as written.
    pub headers: Vec<(String, String)>,
}

impl Envelope {
    pub fn new(from: &str, to: &str, subject: &str, list_id: &str) -> Self {
        Envelope {
            from: from.to_lowercase(),
            to: to.to_lowercase(),
            subject: subject.to_lowercase(),
            list_id: list_id.to_lowercase(),
            ..Default::default()
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
    pub fn from_message(parsed: &petrel_mime::ParsedMessage, size: u64) -> Self {
        fn named(display: Option<&str>, addr: &str) -> String {
            match display {
                Some(d) if !d.is_empty() => format!("{d} {addr}"),
                _ => addr.to_string(),
            }
        }
        fn listed(addrs: &[(Option<String>, String)]) -> String {
            addrs
                .iter()
                .map(|(display, addr)| named(display.as_deref(), addr))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Envelope {
            from: named(
                parsed.from_display.as_deref(),
                parsed.from_addr.as_deref().unwrap_or(""),
            )
            .to_lowercase(),
            to: listed(&parsed.to).to_lowercase(),
            cc: listed(&parsed.cc).to_lowercase(),
            subject: parsed.subject.as_deref().unwrap_or("").to_lowercase(),
            list_id: parsed.list_id.as_deref().unwrap_or("").to_lowercase(),
            // The indexed text, not the raw body: matching markup would let a
            // rule for "invoice" fire on a message whose only "invoice" was a
            // CSS class name.
            body: parsed.index_text().to_lowercase(),
            size,
            date_ms: parsed.date_ms.unwrap_or(0),
            headers: parsed.headers.clone(),
        }
    }
}

/// Whether one text field satisfies one condition.
///
/// The needle is lowercased here and the haystacks were lowercased when the
/// envelope was built, so every test is case-insensitive — which is what a
/// person means by "subject is Invoice", and what every other client does.
fn text_holds(hay: &str, op: Op, needle: &str) -> bool {
    // A half-written rule matches nothing, whichever way it is phrased. The
    // positive operators would match every message on an empty needle and the
    // negative ones would too — "subject does not contain ''" is true of all
    // mail — so the guard has to come before the negation, not after it.
    if needle.is_empty() {
        return false;
    }
    let needle = needle.to_lowercase();
    let plain = match op {
        Op::Contains | Op::NotContains => hay.contains(&needle),
        Op::Is | Op::IsNot => hay == needle,
        Op::StartsWith | Op::NotStartsWith => hay.starts_with(&needle),
        Op::EndsWith | Op::NotEndsWith => hay.ends_with(&needle),
        // Asking a number's question of a string. Never true, rather than
        // true by accident: an editor cannot offer this pairing, so reaching
        // it means a rule from a newer build.
        Op::Over | Op::Under | Op::Before | Op::After => return false,
    };
    plain != op.negated()
}

/// Midnight UTC on a `YYYY-MM-DD`, for comparing against a send time.
///
/// Days rather than instants because that is the granularity of the question:
/// nobody writes a rule about mail sent before half past two. UTC because the
/// stored date is, and quietly using the reader's zone would make the same
/// rule mean different things on two machines.
fn day_start_ms(ymd: &str) -> Option<i64> {
    let mut parts = ymd.trim().split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from the civil calendar to the epoch — Howard Hinnant's algorithm,
    // which needs no table and no leap-year special cases.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400_000)
}

/// Whether every condition holds. A rule with no conditions matches nothing:
/// an empty rule that matched everything would refile the whole inbox the
/// moment it was half-created.
pub fn matches(rule: &Rule, envelope: &Envelope) -> bool {
    if !rule.enabled || rule.conditions.is_empty() {
        return false;
    }
    rule.conditions.iter().all(|c| holds(c, envelope))
}

fn holds(c: &Condition, envelope: &Envelope) -> bool {
    match c.field.as_str() {
        "from" => text_holds(&envelope.from, c.op, &c.value),
        "to" => text_holds(&envelope.to, c.op, &c.value),
        "cc" => text_holds(&envelope.cc, c.op, &c.value),
        "subject" => text_holds(&envelope.subject, c.op, &c.value),
        "list_id" => text_holds(&envelope.list_id, c.op, &c.value),
        "body" => text_holds(&envelope.body, c.op, &c.value),
        "header" => {
            let Some(name) = c.header.as_deref().map(str::to_ascii_lowercase) else {
                return false;
            };
            // A header can appear more than once, and a rule that names one
            // means any of them. The negative operators are the reason this
            // cannot simply be `any`: "X-Spam-Flag does not contain YES" has
            // to hold for *every* copy, or a message with two of them passes
            // on the strength of the innocent one.
            for (_, v) in envelope.headers.iter().filter(|(n, _)| *n == name) {
                // `text_holds` has already applied the negation, so `held`
                // means "this copy satisfies the condition".
                let held = text_holds(&v.to_lowercase(), c.op, &c.value);
                if c.op.negated() {
                    if !held {
                        return false;
                    }
                } else if held {
                    return true;
                }
            }
            // Falling out means every copy passed a negative test, or no copy
            // passed a positive one — and an absent header lands here too,
            // which is the same answer for the same reason: a header that is
            // not there does not contain anything.
            c.op.negated()
        }
        "size" => {
            // Kilobytes, because that is the unit the number is typed in.
            let Ok(kb) = c.value.trim().parse::<u64>() else {
                return false;
            };
            let bytes = kb.saturating_mul(1024);
            match c.op {
                Op::Over => envelope.size > bytes,
                Op::Under => envelope.size < bytes,
                _ => false,
            }
        }
        "date" => {
            let Some(at) = day_start_ms(&c.value) else {
                return false;
            };
            match c.op {
                // "Before the 5th" excludes the 5th; "after the 5th" starts
                // at the 6th. Anything else makes a pair of rules that should
                // partition the calendar overlap on one day.
                Op::Before => envelope.date_ms < at,
                Op::After => envelope.date_ms >= at + 86_400_000,
                _ => false,
            }
        }
        // A field this build does not know (written by a newer one): never
        // match, rather than matching everything.
        _ => false,
    }
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
                    header: None,
                    op: Op::Contains,
                    value: c.to_string(),
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
        let env = Envelope::from_message(&parsed, raw.len() as u64);

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
