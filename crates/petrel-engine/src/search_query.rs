//! The search field's grammar.
//!
//! One string holds everything: `from:sam has:attachment annex` is words to
//! look for plus conditions to meet, and the whole of it is visible and
//! editable. The chips above the field write into this string rather than
//! keeping a parallel state of their own — someone who never learns the
//! grammar gets buttons, someone who does gets the same thing faster, and
//! neither is fighting a filter they cannot see.
//!
//! Deliberately forgiving. An operator nobody recognises stays in the search
//! text rather than being rejected: `re:pricing` is a subject line far more
//! often than a failed attempt at an operator, and a field that argues with
//! what you typed is worse than one that searches for it.

/// A parsed query: what to look for, and what has to be true of it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// The words, with the operators removed. Empty means "everything that
    /// meets the conditions", which is a legitimate search: `has:attachment`
    /// on its own is a question worth asking.
    pub text: String,
    /// Matched against the sender's name and address, case-insensitively.
    pub from: Option<String>,
    pub has_attachment: bool,
    pub unread: bool,
    pub starred: bool,
    /// Put-aside mail. In the grammar so the Snoozed view's search can scope
    /// itself the way every other view's does.
    pub snoozed: bool,
    /// A mailbox role — inbox, sent, archive and the rest.
    pub in_role: Option<String>,
    /// Only mail on or after this instant.
    pub after_ms: Option<i64>,
}

impl SearchQuery {
    /// Whether anything at all was asked for. An empty query is not a search.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.from.is_none()
            && !self.has_attachment
            && !self.unread
            && !self.starred
            && !self.snoozed
            && self.in_role.is_none()
            && self.after_ms.is_none()
    }

    /// True when only conditions were given, with no words to rank by.
    pub fn conditions_only(&self) -> bool {
        self.text.trim().is_empty() && !self.is_empty()
    }
}

/// Splits on whitespace, but keeps `from:"Dana Wu"` together.
fn tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// January the first of a year, in milliseconds.
///
/// Only whole years, because that is the only date arithmetic worth doing
/// without a calendar library: "this year" is the question people actually ask
/// of a mailbox, and anything finer is better expressed by scrolling.
fn year_start_ms(year: i64) -> Option<i64> {
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            366
        } else {
            365
        };
    }
    Some(days * 86_400_000)
}

/// Reads the field.
pub fn parse(input: &str) -> SearchQuery {
    let mut q = SearchQuery::default();
    let mut words: Vec<String> = Vec::new();

    for token in tokens(input) {
        let (key, value) = match token.split_once(':') {
            Some((k, v)) if !v.is_empty() => (k.to_ascii_lowercase(), v.to_string()),
            // A bare word, or a trailing colon somebody is mid-way through
            // typing. Either way it is text, not a condition.
            _ => {
                words.push(token);
                continue;
            }
        };

        match (key.as_str(), value.to_ascii_lowercase().as_str()) {
            ("from", _) => q.from = Some(value),
            ("has", "attachment" | "attachments" | "file") => q.has_attachment = true,
            ("is", "unread") => q.unread = true,
            ("is", "read") => {
                // Deliberately not a field of its own: "is:read" is rare, and a
                // second boolean that only ever means "not the first one" is a
                // way to end up asking for both at once.
                q.unread = false;
            }
            ("is", "starred" | "flagged") => q.starred = true,
            ("is", "snoozed") => q.snoozed = true,
            ("in", _) => q.in_role = Some(value.to_ascii_lowercase()),
            ("after", _) => match value.parse::<i64>().ok().and_then(year_start_ms) {
                Some(ms) => q.after_ms = Some(ms),
                // Not a year we understand. Keep it as text rather than
                // silently dropping the term someone typed.
                None => words.push(token),
            },
            // An operator we do not know. `re:pricing` is a subject far more
            // often than a mistake, so it searches rather than erroring.
            _ => words.push(token),
        }
    }

    q.text = words.join(" ");
    q
}

/// The token a chip writes into the field.
pub fn token_for(chip: &str) -> Option<&'static str> {
    match chip {
        "attachment" => Some("has:attachment"),
        "unread" => Some("is:unread"),
        "starred" => Some("is:starred"),
        "inbox" => Some("in:inbox"),
        _ => None,
    }
}
