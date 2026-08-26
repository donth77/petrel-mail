//! The calendar part of an invitation email, read for the card.
//!
//! Not a general iCalendar implementation. This reads the one VEVENT an
//! invitation carries — Google-style `METHOD:REQUEST` with organizer and
//! attendees, and the method-less event attachments booking tools send —
//! and hands the reader what its card shows: when, where, who is asking,
//! and whether this recipient has standing to answer. Deliberately total,
//! like the message parser beside it: a malformed calendar yields whatever
//! could be read, never a refusal to show mail the user already has.

use mail_parser::MimeHeaders;

/// When an event happens, as precisely as the invitation said it.
///
/// Three shapes, kept apart rather than collapsed: a `...Z` instant is a
/// fact convertible to the reader's clock; a `TZID=...` local time is only
/// displayable beside its zone's name without a timezone database; a bare
/// date is an all-day event and showing a clock time for it would invent
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcalTime {
    /// An instant, epoch milliseconds — from `20190222T233000Z`.
    Utc(i64),
    /// A wall-clock time in a named zone — from
    /// `DTSTART;TZID=America/New_York:20250820T113000`.
    Local { raw: String, tzid: Option<String> },
    /// An all-day date, `YYYYMMDD`.
    Date(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Person {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attendee {
    pub name: Option<String>,
    pub email: Option<String>,
    /// NEEDS-ACTION, ACCEPTED, TENTATIVE, DECLINED — as the wire had it.
    pub partstat: Option<String>,
}

/// One invitation, as far as its calendar part said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Invitation {
    /// REQUEST, CANCEL, REPLY. None is a plain event attachment: there is
    /// nobody to answer, so the card renders without buttons.
    pub method: Option<String>,
    pub uid: Option<String>,
    pub sequence: i64,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub organizer: Option<Person>,
    pub attendees: Vec<Attendee>,
    pub start: Option<IcalTime>,
    pub end: Option<IcalTime>,
    /// CONFIRMED, CANCELLED — the event's own word, distinct from METHOD.
    pub status: Option<String>,
    /// An RRULE was present. The card says "repeats" and no more.
    pub recurring: bool,
}

/// The first calendar part in a raw message, parsed.
///
/// Looks at every MIME part, not just attachments: Google sends the
/// calendar as a bare alternative part, booking tools as a named `.ics`
/// attachment, and both are the same invitation.
pub fn invitation_in(raw: &[u8]) -> Option<Invitation> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    for part in msg.parts.iter() {
        let is_calendar = part
            .content_type()
            .map(|ct| {
                ct.ctype().eq_ignore_ascii_case("text")
                    && ct
                        .subtype()
                        .is_some_and(|s| s.eq_ignore_ascii_case("calendar"))
            })
            .unwrap_or(false)
            || part
                .attachment_name()
                .is_some_and(|n| n.to_ascii_lowercase().ends_with(".ics"));
        if !is_calendar {
            continue;
        }
        let text = String::from_utf8_lossy(part.contents());
        if let Some(inv) = parse_invitation(&text) {
            return Some(inv);
        }
    }
    None
}

/// Parses one VCALENDAR text into the invitation it carries.
pub fn parse_invitation(ics: &str) -> Option<Invitation> {
    let mut inv = Invitation::default();
    let mut saw_vevent = false;
    // Where we are in the component tree: properties only count at the
    // levels that own them. A VTIMEZONE carries DTSTART lines of its own,
    // and reading those as the event's start was the first bug this stack
    // exists to prevent.
    let mut stack: Vec<String> = Vec::new();
    for line in unfold(ics) {
        let Some((name, params, value)) = content_line(&line) else {
            continue;
        };
        let upper = name.to_ascii_uppercase();
        match upper.as_str() {
            "BEGIN" => {
                stack.push(value.to_ascii_uppercase());
                if stack.last().map(String::as_str) == Some("VEVENT") && stack.len() == 2 {
                    if saw_vevent {
                        // One card, one event: later VEVENTs (recurrence
                        // exceptions) are beyond what the card says.
                        break;
                    }
                    saw_vevent = true;
                }
                continue;
            }
            "END" => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        let in_calendar = stack.last().map(String::as_str) == Some("VCALENDAR");
        let in_event = stack.last().map(String::as_str) == Some("VEVENT") && saw_vevent;
        if in_calendar && upper == "METHOD" {
            inv.method = Some(value.trim().to_ascii_uppercase());
        }
        if !in_event {
            continue;
        }
        match upper.as_str() {
            "UID" => inv.uid = Some(value.trim().to_string()),
            "SEQUENCE" => inv.sequence = value.trim().parse().unwrap_or(0),
            "SUMMARY" => inv.summary = non_empty(unescape(&value)),
            "LOCATION" => inv.location = non_empty(unescape(&value)),
            "DESCRIPTION" => inv.description = non_empty(unescape(&value)),
            "STATUS" => inv.status = Some(value.trim().to_ascii_uppercase()),
            "RRULE" => inv.recurring = true,
            "DTSTART" => inv.start = Some(parse_time(&params, &value)),
            "DTEND" => inv.end = Some(parse_time(&params, &value)),
            "ORGANIZER" => {
                inv.organizer = Some(Person {
                    name: param_value(&params, "CN"),
                    email: mailto(&value),
                })
            }
            "ATTENDEE" => inv.attendees.push(Attendee {
                name: param_value(&params, "CN"),
                email: mailto(&value),
                partstat: param_value(&params, "PARTSTAT").map(|p| p.to_ascii_uppercase()),
            }),
            _ => {}
        }
    }
    saw_vevent.then_some(inv)
}

/// Folded lines rejoined: a line starting with space or tab continues the
/// one before it (RFC 5545 §3.1).
fn unfold(ics: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in ics.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        if let Some(rest) = raw.strip_prefix([' ', '\t'])
            && let Some(last) = out.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        out.push(raw.to_string());
    }
    out
}

/// `NAME;PARAM="a;b";OTHER=x:VALUE` → (name, params, value). Quotes guard
/// both `;` and `:`, which real CN values use freely.
/// A property's parameters: `(NAME, value)` pairs, quotes already shed.
type Params = Vec<(String, String)>;

fn content_line(line: &str) -> Option<(String, Params, String)> {
    let mut in_quotes = false;
    let mut split = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (head, value) = line.split_at(split?);
    let value = &value[1..];
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in head.chars() {
        match c {
            '"' => q = !q,
            ';' if !q => {
                segs.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        if c != '"' {
            cur.push(c);
        }
    }
    segs.push(cur);
    let name = segs.first()?.clone();
    let params = segs[1..]
        .iter()
        .filter_map(|s| {
            let (k, v) = s.split_once('=')?;
            Some((k.to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    Some((name, params, value.to_string()))
}

fn param_value(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .and_then(non_empty)
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn mailto(value: &str) -> Option<String> {
    let v = value.trim();
    let rest = v
        .strip_prefix("mailto:")
        .or_else(|| v.strip_prefix("MAILTO:"))
        .unwrap_or(v);
    non_empty(rest.to_string()).filter(|s| s.contains('@'))
}

/// `\n \, \; \\` written out (RFC 5545 §3.3.11).
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn parse_time(params: &[(String, String)], value: &str) -> IcalTime {
    let v = value.trim();
    if params
        .iter()
        .any(|(k, val)| k == "VALUE" && val.eq_ignore_ascii_case("DATE"))
        || (v.len() == 8 && v.bytes().all(|b| b.is_ascii_digit()))
    {
        return IcalTime::Date(v.to_string());
    }
    if let Some(stripped) = v.strip_suffix('Z')
        && let Some(ms) = utc_ms(stripped)
    {
        return IcalTime::Utc(ms);
    }
    IcalTime::Local {
        raw: v.to_string(),
        tzid: param_value(params, "TZID"),
    }
}

/// `YYYYMMDDTHHMMSS` → epoch milliseconds, by the civil-days arithmetic —
/// no timezone crate needed for a value that is already UTC.
fn utc_ms(v: &str) -> Option<i64> {
    let b = v.as_bytes();
    if b.len() != 15 || b[8] != b'T' {
        return None;
    }
    let num = |s: &str| s.parse::<i64>().ok();
    let (y, mo, d) = (num(&v[0..4])?, num(&v[4..6])?, num(&v[6..8])?);
    let (h, mi, s) = (num(&v[9..11])?, num(&v[11..13])?, num(&v[13..15])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    // Howard Hinnant's days-from-civil.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3_600 + mi * 60 + s) * 1_000)
}
