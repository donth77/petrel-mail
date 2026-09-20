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
//!
//! The operators are the table in docs 07 §5.1, and that table is canonical.
//! What this file adds is the shape: a small tree. A space is AND, `OR` is
//! either side, `-` or `NOT` excludes, and brackets group — evaluated in the
//! order brackets, NOT, AND, OR, which is the order every Boolean search a
//! person has met uses. The tree is bounded in every direction it can grow.

use std::fmt;

/// How many separate statements one query may become in the store.
///
/// Words joined by `OR` are one full-text expression and conditions joined
/// by `OR` are one SQL predicate, so most queries are a single statement
/// however they are bracketed. Only an `OR` with words on one side and a
/// condition on the other has to be asked twice, and a query built to
/// multiply those must not become a hundred statements. One that would pass
/// this is not cut short: the store asks it whole, as one statement that
/// cannot rank its results (`as_lookups` in `store/search.rs`).
///
/// Four, by measurement at a hundred thousand messages with a word that is
/// in half of them. Each statement ranks its own words, so the cost is about
/// the sum: one took 97ms, four 162ms, and eight 226ms, which is over the
/// 200ms a search is allowed. The same eight asked whole took 32ms. Two
/// bracketed choices of a word or a condition still rank; a third is where
/// ranking stops paying for itself.
pub const MAX_ALTERNATIVES: usize = 4;
/// How many clauses a whole query can hold. Each binds a few SQL variables,
/// and a condition that matches little reads every message: thirty-two
/// senders joined by `OR` that match nobody took 295ms at a hundred thousand
/// messages. Past the limits the rest is left out, and the parse says so in
/// `truncated` rather than pretending it read everything.
///
/// The search field keeps a copy of this and of `MAX_VALUE_CHARS`
/// (`search-limits.ts`), to tell the person typing when their query was cut.
/// A test in the desktop crate holds the copy to these.
pub const MAX_CLAUSES: usize = 32;
/// How long one value can be. Nobody searches for a 300-character name, and
/// SQLite refuses a LIKE pattern that runs to tens of thousands of bytes.
pub const MAX_VALUE_CHARS: usize = 256;
// Brackets have no limit of their own. The clause limit is one: the tree is
// kept tidy, so every level of it holds at least two things, and one with
// thirty-two clauses in it cannot be much deeper than that however many
// brackets were typed. Nothing that reads the field calls itself per bracket.

/// A parsed query.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// What was asked, or nothing: an empty query is not a search.
    pub root: Option<Expr>,
    /// True when the field held more than the limits allow.
    pub truncated: bool,
}

/// What a query is made of. The parser only ever builds the tidy form: an
/// `All` never directly holds an `All`, nor an `Any` an `Any`, neither holds
/// fewer than two things, a `Not` holds one of those groups rather than a
/// single term, and an excluded single term is a negated `Clause`. So a tree
/// cannot be deeper than the clauses in it, however the field was bracketed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Clause(Clause),
    /// A bracketed group that must not hold: `-(a OR b)`, `NOT (a b)`.
    Not(Box<Expr>),
    /// Every part has to hold. AND is the default, and never OR.
    All(Vec<Expr>),
    /// Any part may hold.
    Any(Vec<Expr>),
}

/// One thing asked of a message, or with `-` in front, one thing it must not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub negated: bool,
    pub term: Term,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    /// Words to find anywhere in the message.
    Text(Text),
    /// Words to find in the subject line and nowhere else.
    Subject(Text),
    /// Matched against the sender's name and address, case-insensitively.
    From(String),
    /// Anyone in To.
    To(String),
    /// Anyone copied.
    Cc(String),
    /// A mailbox role — inbox, sent, archive and the rest — or a folder the
    /// user made, by its path or its last part. Held lowercased, because the
    /// roles are, and the store asks `In("spam")` by name.
    In(String),
    /// A tag, by its whole name.
    Tag(String),
    /// Part of an attached file's name.
    Filename(String),
    Is(State),
    HasAttachment,
    /// Sent on or after the start of the period.
    After(Period),
    /// Sent before the start of the period.
    Before(Period),
    /// Sent within the period.
    On(Period),
}

/// Words, and whether they were typed inside quotes.
///
/// Quoted means exactly these words in this order, and it also means
/// finished: the as-you-type prefix goes on a word somebody may still be
/// typing, never on one they closed the quotes around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub value: String,
    pub exact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unread,
    /// A state of its own rather than "not unread". The old parser let
    /// `is:read` cancel an earlier `is:unread`, which made `is:read` alone
    /// not a search at all. With real negation in the grammar the honest
    /// reading of asking for both is that nothing is both.
    Read,
    Starred,
    /// Put-aside mail. In the grammar so the Snoozed view's search can scope
    /// itself the way every other view's does.
    Snoozed,
}

/// One calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Day {
    /// Midnight at the start of this day in UTC, in milliseconds.
    ///
    /// The store asks SQLite for the *local* midnight, because 07 §5.1 says
    /// a date means that calendar day where the reader is. This is what it
    /// falls back on if SQLite cannot say, and what the tests check the
    /// calendar arithmetic against.
    pub fn utc_ms(&self) -> i64 {
        // Days from 1970-01-01, counted in 400-year eras that start in March
        // so the leap day is the last day of the year it belongs to.
        let y = i64::from(self.year) - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let year_of_era = y.rem_euclid(400);
        let month = i64::from(self.month);
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5
            + i64::from(self.day)
            - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        (era * 146_097 + day_of_era - 719_468) * 86_400_000
    }
}

fn days_in(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        _ => 28,
    }
}

/// A year, a month or a day: `2026`, `2026-08`, `2026-08-14`.
///
/// The fields are private so that one of these always names a real stretch
/// of the calendar. `after:2026-02-30` is not a date, and stays text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    year: u16,
    month: Option<u8>,
    day: Option<u8>,
}

impl Period {
    /// 9998 rather than 9999, so that the day after a period is always a
    /// date SQLite can still name.
    const YEARS: std::ops::RangeInclusive<u16> = 1970..=9998;

    pub fn year(year: u16) -> Option<Period> {
        Self::YEARS.contains(&year).then_some(Period {
            year,
            month: None,
            day: None,
        })
    }

    pub fn month(year: u16, month: u8) -> Option<Period> {
        let of = Self::year(year)?;
        (1..=12).contains(&month).then_some(Period {
            month: Some(month),
            ..of
        })
    }

    pub fn day(year: u16, month: u8, day: u8) -> Option<Period> {
        let of = Self::month(year, month)?;
        (1..=days_in(year, month)).contains(&day).then_some(Period {
            day: Some(day),
            ..of
        })
    }

    /// ISO, as 07 §5.1 says, and `/` for `-` because that is what people
    /// arriving from Gmail type. Whole years were the only form the first
    /// parser took, and the year chip still writes one.
    ///
    /// A separator with nothing after it yet — `2026-`, `2026-08-` — is the
    /// date so far, so the results do not blink out between the month and
    /// the day. Anything actually wrong, `2026-13`, is not a date at all.
    fn parse(value: &str) -> Option<Period> {
        let mut parts: Vec<&str> = value.split(['-', '/']).collect();
        if parts.len() > 1 && parts.last() == Some(&"") {
            parts.pop();
        }
        let number = |part: &str, digits: std::ops::RangeInclusive<usize>| {
            (digits.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse::<u16>().ok())
                .flatten()
        };
        let small = |part: &str| u8::try_from(number(part, 1..=2)?).ok();
        match parts[..] {
            [year] => Self::year(number(year, 4..=4)?),
            [year, month] => Self::month(number(year, 4..=4)?, small(month)?),
            [year, month, day] => Self::day(number(year, 4..=4)?, small(month)?, small(day)?),
            _ => None,
        }
    }

    /// The first day inside the period.
    pub fn first_day(&self) -> Day {
        Day {
            year: self.year,
            month: self.month.unwrap_or(1),
            day: self.day.unwrap_or(1),
        }
    }

    /// The first day after it.
    pub fn day_after(&self) -> Day {
        let first = self.first_day();
        match (self.month, self.day) {
            (None, _) | (Some(12), None) => Day {
                year: first.year + 1,
                month: 1,
                day: 1,
            },
            (Some(month), None) => Day {
                month: month + 1,
                ..first
            },
            (Some(month), Some(day)) if day < days_in(first.year, month) => Day {
                day: day + 1,
                ..first
            },
            (Some(month), Some(_)) => Period {
                year: first.year,
                month: Some(month),
                day: None,
            }
            .day_after(),
        }
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}", self.year)?;
        if let Some(month) = self.month {
            write!(f, "-{month:02}")?;
        }
        if let Some(day) = self.day {
            write!(f, "-{day:02}")?;
        }
        Ok(())
    }
}

impl SearchQuery {
    /// Whether anything at all was asked for. An empty query is not a search.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
}

/// One whitespace-separated piece of the field, with its quote marks taken
/// off and a note of where the first one was.
struct Piece {
    text: String,
    /// How much of `text` came before the first quote mark. `Some(0)` is a
    /// phrase, `from:"Dana Wu"` is `Some(5)`, and a bare word is `None`.
    quote_at: Option<usize>,
}

enum Lexeme {
    Open,
    Close,
    /// The `-` of `-(`: it excludes the group, and is never a word.
    Minus,
    /// The `from:` of `from:(sam OR dana)`: the operator the group is given.
    Shared(String),
    Piece(Piece),
}

const KEYWORDS: [&str; 3] = ["AND", "OR", "NOT"];

/// The operator a `(` is about to be given, if it is: `from:(`, `-tag:(`.
fn shared_key(text: &str) -> Option<String> {
    let key = text.trim_start_matches('-').strip_suffix(':')?;
    let key = key.to_ascii_lowercase();
    SHARED.contains(&key.as_str()).then_some(key)
}

/// Splits on whitespace, but keeps `from:"Dana Wu"` and `"board pack"`
/// together, and takes brackets off the ends of words.
///
/// Curly quotes count as quotes: macOS swaps them in for straight ones in
/// plenty of places a query might be copied from, and they mean nothing else
/// here. A quote that is never closed runs to the end, which is what a
/// phrase looks like while it is still being typed.
///
/// A bracket groups only where it could not be part of a word: opening at
/// the start of one or after an operator's colon, closing at the end of one
/// and only while a bracket is open. So `foo(bar` and `a)b` are text, and so
/// are `fn(x)` and `:)`, which closed a group nobody opened until it was
/// noticed that `alpha OR bravo happy :) done` quietly became
/// `(alpha OR bravo happy :) done`.
fn lex(input: &str) -> Vec<Lexeme> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut quote_at = None;
    let mut quoted = false;
    fn flush(out: &mut Vec<Lexeme>, text: &mut String, quote_at: &mut Option<usize>) {
        let quote_at = quote_at.take();
        if !text.is_empty() {
            out.push(Lexeme::Piece(Piece {
                text: std::mem::take(text),
                quote_at,
            }));
        }
    }
    // What is open, so that a `)` only closes something that exists.
    let mut depth = 0usize;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' | '\u{201C}' | '\u{201D}' | '\u{201E}' => {
                quoted = !quoted;
                quote_at.get_or_insert(text.len());
            }
            // Control characters break FTS5's own string parser even inside
            // quotes, so they are spacing here before they are anything else.
            c if c.is_whitespace() || c.is_control() => {
                if !quoted {
                    flush(&mut out, &mut text, &mut quote_at);
                } else if !text.ends_with(' ') {
                    // One space however it was typed, so `from:"Dana  Wu"`
                    // still finds Dana Wu.
                    text.push(' ');
                }
            }
            '(' if !quoted && quote_at.is_none() && text.is_empty() => {
                depth += 1;
                out.push(Lexeme::Open);
            }
            // However many dashes, one exclusion: `--(` is `-(`, as `--draft`
            // is `-draft`.
            '(' if !quoted && quote_at.is_none() && text.bytes().all(|b| b == b'-') => {
                text.clear();
                depth += 1;
                out.push(Lexeme::Minus);
                out.push(Lexeme::Open);
            }
            // `from:(sam OR dana)`, which is how Gmail writes it. The group
            // is read as usual and every word in it is given the operator,
            // so this is `from:sam OR from:dana` and writes back as that.
            '(' if !quoted && quote_at.is_none() && shared_key(&text).is_some() => {
                let dashes = text.len() - text.trim_start_matches('-').len();
                let key = shared_key(&text).expect("just tested");
                text.clear();
                depth += 1;
                if dashes > 0 {
                    out.push(Lexeme::Minus);
                }
                out.push(Lexeme::Shared(key));
                out.push(Lexeme::Open);
            }
            // `NOT(a OR b)`, with no space, is what half of everyone types.
            '(' if !quoted && quote_at.is_none() && KEYWORDS.contains(&text.as_str()) => {
                flush(&mut out, &mut text, &mut quote_at);
                depth += 1;
                out.push(Lexeme::Open);
            }
            // A control character is spacing (above), so it ends a word here
            // as well: `(a OR b)` closes whether a space or a stray NUL
            // follows it.
            ')' if !quoted
                && depth > 0
                && chars.peek().is_none_or(|next| {
                    *next == ')' || next.is_whitespace() || next.is_control()
                }) =>
            {
                depth -= 1;
                flush(&mut out, &mut text, &mut quote_at);
                out.push(Lexeme::Close);
            }
            c => text.push(c),
        }
    }
    flush(&mut out, &mut text, &mut quote_at);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Keyword {
    And,
    Or,
    Not,
}

/// What one piece of the field turned out to be.
enum Read {
    Keyword(Keyword),
    Clause(Clause),
    /// Nothing: an empty pair of quotes.
    Nothing,
}

/// Trims a value and holds it to the length limit.
fn value_of(raw: &str, truncated: &mut bool) -> String {
    let raw = raw.trim();
    if raw.chars().count() > MAX_VALUE_CHARS {
        *truncated = true;
        // Trimmed again: the cut can land just after a space, and a value
        // that ends in one would not read back as itself.
        let cut: String = raw.chars().take(MAX_VALUE_CHARS).collect();
        return cut.trim_end().to_string();
    }
    raw.to_string()
}

/// What an operator and its value mean, or `None` for an operator nobody
/// knows and for a value one cannot take. Those search rather than erroring,
/// and rather than silently dropping the term somebody typed.
fn term_for(key: &str, value: &str, exact: bool, truncated: &mut bool) -> Option<Term> {
    let words = || Text {
        value: value.to_string(),
        exact,
    };
    match (key, value.to_ascii_lowercase().as_str()) {
        ("from", _) => Some(Term::From(value.to_string())),
        ("to", _) => Some(Term::To(value.to_string())),
        ("cc", _) => Some(Term::Cc(value.to_string())),
        ("subject", _) => Some(Term::Subject(words())),
        // Held to the limit again after lowering: a few letters lower to two
        // (`İ`), so a value at the limit could leave here over it.
        ("in", _) => Some(Term::In(value_of(&value.to_lowercase(), truncated))),
        ("tag", _) => Some(Term::Tag(value.to_string())),
        ("filename", _) => Some(Term::Filename(value.to_string())),
        ("has", "attachment" | "attachments" | "file") => Some(Term::HasAttachment),
        ("is", "unread") => Some(Term::Is(State::Unread)),
        ("is", "read") => Some(Term::Is(State::Read)),
        ("is", "starred" | "flagged") => Some(Term::Is(State::Starred)),
        ("is", "snoozed") => Some(Term::Is(State::Snoozed)),
        ("after", when) => Period::parse(when).map(Term::After),
        ("before", when) => Period::parse(when).map(Term::Before),
        ("date", when) => Period::parse(when).map(Term::On),
        _ => None,
    }
}

/// The operators a bracket can be shared between: `from:(sam OR dana)` is
/// `from:sam OR from:dana`, which is how Gmail writes it and how people who
/// have used Gmail write it here.
///
/// Every operator that takes a word or a state. The three dates are left out:
/// each of `after:` and `before:` narrows from one end, so a bracketed list of
/// them is either one date doing all the work or a contradiction, and `date:`
/// wants `OR` between whole days about as often as never. A bracket after one
/// of those stays an ordinary group, which is what it was before.
const SHARED: [&str; 9] = [
    "from", "to", "cc", "subject", "in", "tag", "filename", "is", "has",
];

/// That operator, given to every word inside the group it opened.
///
/// The truncation flag comes through because `term_for` can still cut a value
/// here: `in:` lowercases before holding it to the limit, and a few letters
/// lower to two. Ignored, `in:(İ…)` said nothing was cut while the same value
/// written `in:İ…` said it was, and the field's own notice disagreed with the
/// engine about the query it had just run.
fn applied(key: &str, expr: Expr, truncated: &mut bool) -> Expr {
    match expr {
        Expr::Clause(Clause {
            negated,
            term: Term::Text(text),
        }) => {
            let term =
                term_for(key, &text.value, text.exact, truncated).unwrap_or(Term::Text(text));
            Expr::Clause(Clause { negated, term })
        }
        // Already an operator of its own: `from:(sam OR to:dana)` means what
        // it says rather than `to:to:dana`.
        Expr::Clause(clause) => Expr::Clause(clause),
        Expr::Not(inner) => Expr::Not(Box::new(applied(key, *inner, truncated))),
        Expr::All(parts) => Expr::All(
            parts
                .into_iter()
                .map(|p| applied(key, p, truncated))
                .collect(),
        ),
        Expr::Any(parts) => Expr::Any(
            parts
                .into_iter()
                .map(|p| applied(key, p, truncated))
                .collect(),
        ),
    }
}

fn read(piece: Piece, truncated: &mut bool) -> Read {
    let Piece {
        mut text,
        mut quote_at,
    } = piece;

    // Capitals only, and always. People type "or" and "not" in ordinary
    // searches — `now or never` is three words — and the literal word is
    // still reachable in quotes, as `"OR"`.
    //
    // No guessing beyond that. A pasted `DO NOT REPLY` does read as Boolean,
    // and a rule that tried to tell it from `IBM OR HP` by the capitals around
    // it would be reading intent off typography. The field offers the quoted
    // reading as a chip instead (`otherReading` in search-chips.ts), which is
    // the same fix in the hands of the one person who knows what was meant.
    if quote_at.is_none() {
        match text.as_str() {
            "AND" => return Read::Keyword(Keyword::And),
            "OR" => return Read::Keyword(Keyword::Or),
            "NOT" => return Read::Keyword(Keyword::Not),
            _ => {}
        }
    }

    // A leading `-` outside quotes excludes. `e-mail` has its hyphen inside
    // the word, `"-5"` has its inside the quotes, and a lone `-` is a dash.
    //
    // However many there are, they are one exclusion: `--draft` is `-draft`.
    // Nothing is left that begins with a minus and is not excluded, which is
    // what lets NOT be taken off again and the word still be written bare.
    let dashes = match quote_at {
        Some(at) => text[..at].len() - text[..at].trim_start_matches('-').len(),
        None => text.len() - text.trim_start_matches('-').len(),
    };
    let negated = dashes > 0 && dashes < text.len();
    if negated {
        text.drain(..dashes);
        quote_at = quote_at.map(|at| at - dashes);
    }
    let clause = |term| Read::Clause(Clause { negated, term });
    let exact = quote_at.is_some();

    // An operator is a known word, a colon outside the quotes, and a value.
    // Anything less is text: `"from:sam"` in quotes, a `from:` somebody is
    // mid-way through typing, `re:pricing`, `is:whatever`, `after:soon`.
    let operator = text
        .split_once(':')
        .filter(|(key, _)| quote_at.is_none_or(|at| key.len() < at))
        .map(|(key, value)| (key.to_ascii_lowercase(), value_of(value, truncated)))
        .filter(|(_, value)| !value.is_empty());
    if let Some((key, value)) = operator
        && let Some(term) = term_for(&key, &value, exact, truncated)
    {
        return clause(term);
    }

    let value = value_of(&text, truncated);
    if value.is_empty() {
        return Read::Nothing;
    }
    clause(Term::Text(Text { value, exact }))
}

enum Token {
    Open,
    Close,
    Minus,
    /// The operator the next bracket is given.
    Shared(String),
    Keyword(Keyword),
    Clause(Clause),
}

/// Flips what an expression asks for.
fn negate(expr: Expr) -> Expr {
    match expr {
        Expr::Clause(clause) => Expr::Clause(Clause {
            negated: !clause.negated,
            ..clause
        }),
        Expr::Not(inner) => *inner,
        group => Expr::Not(Box::new(group)),
    }
}

/// Parts that all have to hold, in the tidy form.
fn all_of(parts: Vec<Expr>) -> Option<Expr> {
    let mut flat = Vec::new();
    for part in parts {
        match part {
            Expr::All(inner) => flat.extend(inner),
            other => flat.push(other),
        }
    }
    match flat.len() {
        0 => None,
        1 => flat.pop(),
        _ => Some(Expr::All(flat)),
    }
}

/// Parts any of which may hold, in the tidy form.
fn any_of(parts: Vec<Expr>) -> Option<Expr> {
    let mut flat = Vec::new();
    for part in parts {
        match part {
            Expr::Any(inner) => flat.extend(inner),
            other => flat.push(other),
        }
    }
    match flat.len() {
        0 => None,
        1 => flat.pop(),
        _ => Some(Expr::Any(flat)),
    }
}

/// One bracket's worth of reading: the alternatives finished so far, and the
/// parts of the one being read.
#[derive(Default)]
struct Group {
    /// A NOT or a `-` stood in front of the bracket.
    excluded: bool,
    /// The operator the bracket was given, as in `from:(sam OR dana)`.
    shared: Option<String>,
    alternatives: Vec<Expr>,
    parts: Vec<Expr>,
}

impl Group {
    /// An `OR`: what has been read so far is one alternative, finished.
    fn or(&mut self) {
        let parts = std::mem::take(&mut self.parts);
        self.alternatives.extend(all_of(parts));
    }

    fn close(mut self, truncated: &mut bool) -> Option<Expr> {
        self.or();
        let inner = any_of(self.alternatives);
        let inner = match &self.shared {
            Some(key) => inner.map(|expr| applied(key, expr, truncated)),
            None => inner,
        };
        if self.excluded {
            inner.map(negate)
        } else {
            inner
        }
    }
}

/// Reads tokens in the order brackets, NOT, AND, OR.
///
/// Nothing here can fail. An operator with nothing on one side of it —
/// first, last, or doubled — is what the field holds mid-typing, so it is
/// ignored rather than searched for, or the results would flash empty between
/// one keystroke and the next. A bracket never closed closes at the end.
///
/// A bracket never opened is not a bracket: `lex` calls a `)` a close only
/// while one is open, so `a) OR b` is two words and the first of them ends in
/// a `)`. The reader used to treat a stray one as closing a group taken to have
/// opened at the start, which regrouped everything before it — a smiley in
/// `alpha OR bravo happy :) done` turned the whole beginning into one
/// alternative — so the rule now lives in the lexer, where depth is known.
///
/// Nothing here calls itself, either. The open brackets are a list, and a
/// run of NOTs is a switch that two of them put back. A reader that went one
/// call deeper for each went as deep as the field was long: ten thousand NOTs
/// pasted in overflowed the stack, which is an abort and not an error anyone
/// can catch. The cure for brackets used to be ignoring the ones past a
/// depth, and an ignored bracket is a different query: `p (a OR b) c` nine
/// deep was read as `p a OR b c`. Every bracket counts now, however deep.
fn read_tokens(tokens: Vec<Token>, truncated: &mut bool) -> Option<Expr> {
    let mut open = vec![Group::default()];
    let mut excluding = false;
    let mut sharing: Option<String> = None;
    for token in tokens {
        // Only a word or a bracket can be excluded. A NOT in front of
        // anything else has nothing to exclude yet.
        let excluded = std::mem::take(&mut excluding);
        let innermost = open
            .last_mut()
            .expect("the outermost group is never closed");
        match token {
            Token::Minus | Token::Keyword(Keyword::Not) => excluding = !excluded,
            Token::Keyword(Keyword::And) => {}
            Token::Keyword(Keyword::Or) => innermost.or(),
            Token::Clause(clause) => {
                let clause = Expr::Clause(clause);
                innermost
                    .parts
                    .push(if excluded { negate(clause) } else { clause });
            }
            Token::Shared(key) => {
                sharing = Some(key);
                excluding = excluded;
            }
            Token::Open => open.push(Group {
                excluded,
                shared: sharing.take(),
                ..Group::default()
            }),
            Token::Close => {
                let closed = open.pop().and_then(|group| group.close(truncated));
                // There is always a group left to put it in: a close reaches
                // here only from inside a bracket, so they can never outnumber
                // the opens and the outermost one is never the one popped.
                open.last_mut()
                    .expect("the outermost group is never closed")
                    .parts
                    .extend(closed);
            }
        }
    }
    let mut closed = None;
    while let Some(mut group) = open.pop() {
        group.parts.extend(closed);
        closed = group.close(truncated);
    }
    closed
}

/// Reads the field.
pub fn parse(input: &str) -> SearchQuery {
    let mut q = SearchQuery::default();
    let mut tokens = Vec::new();
    let mut clauses = 0;
    for lexeme in lex(input) {
        let token = match lexeme {
            Lexeme::Open => Token::Open,
            Lexeme::Close => Token::Close,
            Lexeme::Minus => Token::Minus,
            Lexeme::Shared(key) => Token::Shared(key),
            Lexeme::Piece(piece) => match read(piece, &mut q.truncated) {
                Read::Nothing => continue,
                Read::Keyword(keyword) => Token::Keyword(keyword),
                Read::Clause(clause) => Token::Clause(clause),
            },
        };
        if matches!(token, Token::Clause(_)) {
            if clauses == MAX_CLAUSES {
                q.truncated = true;
                break;
            }
            clauses += 1;
        }
        tokens.push(token);
    }

    q.root = read_tokens(tokens, &mut q.truncated);
    if let Some(root) = &mut q.root {
        settle(root);
    }
    q
}

/// A value the way it has to be typed: in quotes when it holds a space, or
/// a `)` where the field would read it as closing a group.
fn typed(value: &str) -> String {
    // Quote marks are taken off on the way in, so none can be in a value
    // that was parsed. One built by hand loses them rather than unbalancing
    // the field.
    let value: String = value.chars().filter(|c| *c != '"').collect();
    // Brackets for the same reason a space is quoted: a folder called
    // `Archive (old)` or a tag `p(1)` written bare closes whatever group it
    // was written inside.
    let brackets = value.contains('(') || value.contains(')');
    if brackets || value.chars().any(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value
    }
}

impl Clause {
    /// Whether these words can go into the field without quotes and come
    /// back out as themselves.
    fn writes_bare(&self) -> bool {
        let Term::Text(text) = &self.term else {
            return true;
        };
        let bare = format!("{}{}", if self.negated { "-" } else { "" }, text.value);
        let mut lexemes = lex(&bare);
        let read_back = match (lexemes.pop(), lexemes.is_empty()) {
            (Some(Lexeme::Piece(piece)), true) => Some(read(piece, &mut false)),
            _ => None,
        };
        // A bracket wears quotes wherever it is. On its own `annex)` reads
        // back as itself, because a `)` closes nothing when nothing is open
        // — but written inside a group, as `-(annex) OR b)`, that same `)`
        // closes the group early. Quoted, it means one thing everywhere.
        //
        // It costs as-you-type completion for such a word: quoted is a phrase,
        // and `typing_word` (store/search/plan.rs) puts its `*` only on a word,
        // so `fn(` stops growing the prefix at the bracket. A word with a
        // bracket in it is rare in mail; a bracket that means two things
        // depending on where it stands is a query that reads as another one.
        !text.exact
            && !bare
                .chars()
                .any(|c| c == '"' || c == '(' || c == ')' || c.is_whitespace())
            && matches!(read_back, Some(Read::Clause(clause)) if clause == *self)
    }
}

/// Words that could only be written in quotes are words in quotes. A NOT
/// taken off a word can leave one no keyboard could have typed bare — the
/// lone dash in `NOT -`, say — and this is what keeps writing a query out
/// and reading it back the identity for those too.
fn settle(expr: &mut Expr) {
    match expr {
        Expr::Clause(clause) => {
            let bare = clause.writes_bare();
            match &mut clause.term {
                Term::Text(text) if !bare => text.exact = true,
                // The same for a subject that can only be written in quotes,
                // which takes a value cut at the length limit just after a
                // `)` to reach.
                Term::Subject(text) if typed(&text.value).starts_with('"') => text.exact = true,
                _ => {}
            }
        }
        Expr::Not(inner) => settle(inner),
        Expr::All(parts) | Expr::Any(parts) => parts.iter_mut().for_each(settle),
    }
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minus = if self.negated { "-" } else { "" };
        if let Term::Text(text) = &self.term {
            // Bare only if it reads back as this same clause. `OR`, `-draft`,
            // `(annex` and `from:sam` as literal text all have to wear quotes.
            return if self.writes_bare() {
                write!(f, "{minus}{}", text.value)
            } else {
                write!(f, "{minus}\"{}\"", typed(&text.value).trim_matches('"'))
            };
        }
        f.write_str(minus)?;
        match &self.term {
            Term::Text(_) => Ok(()),
            Term::Subject(text) if text.exact => {
                write!(f, "subject:\"{}\"", typed(&text.value).trim_matches('"'))
            }
            Term::Subject(text) => write!(f, "subject:{}", typed(&text.value)),
            Term::From(v) => write!(f, "from:{}", typed(v)),
            Term::To(v) => write!(f, "to:{}", typed(v)),
            Term::Cc(v) => write!(f, "cc:{}", typed(v)),
            Term::In(v) => write!(f, "in:{}", typed(v)),
            Term::Tag(v) => write!(f, "tag:{}", typed(v)),
            Term::Filename(v) => write!(f, "filename:{}", typed(v)),
            Term::Is(State::Unread) => f.write_str("is:unread"),
            Term::Is(State::Read) => f.write_str("is:read"),
            Term::Is(State::Starred) => f.write_str("is:starred"),
            Term::Is(State::Snoozed) => f.write_str("is:snoozed"),
            Term::HasAttachment => f.write_str("has:attachment"),
            Term::After(when) => write!(f, "after:{when}"),
            Term::Before(when) => write!(f, "before:{when}"),
            Term::On(when) => write!(f, "date:{when}"),
        }
    }
}

/// Written with the fewest brackets that keep the meaning: `OR` binds
/// loosest, so only an `OR` inside an AND needs them, and an excluded group.
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Clause(clause) => write!(f, "{clause}"),
            Expr::Not(inner) => write!(f, "-({inner})"),
            Expr::All(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    match part {
                        Expr::Any(_) => write!(f, "({part})")?,
                        _ => write!(f, "{part}")?,
                    }
                }
                Ok(())
            }
            Expr::Any(parts) => {
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" OR ")?;
                    }
                    write!(f, "{part}")?;
                }
                Ok(())
            }
        }
    }
}

/// The query as it would be typed. Reading it back gives the same query,
/// which is what lets anything that builds one — a chip, or the layer that
/// reads plain phrases — hand its work to the field as text the person can
/// see and edit.
impl fmt::Display for SearchQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.root {
            Some(root) => write!(f, "{root}"),
            None => Ok(()),
        }
    }
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
