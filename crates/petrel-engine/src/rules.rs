//! Filter rules: triage the user wrote down once, applied on arrival.
//!
//! A rule is conditions over the envelope — who it is from, who it is to,
//! what it is about, which list sent it — and actions that are exactly the
//! triage verbs the app already has. Rules run in the order the user put
//! them, every enabled rule that matches, so two rules can each contribute
//! (one tags, one archives) without a hidden first-match-wins surprise.

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
