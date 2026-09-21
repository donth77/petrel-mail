//! `mailto:` links from outside Petrel, parsed at the door (docs 23 §4).
//!
//! A `mailto:` URL is text a web page chose, and it arrives before any of
//! Petrel's own checks. So it is taken apart here, in the shell, and what
//! reaches the window is already safe to put in a composer: five fields, each
//! one line where it must be, each bounded. Everything else in the URL is
//! dropped on the floor, which is what RFC 6068 §6 asks of a careful client.
//!
//! Nothing here sends anything. A link opens a draft, and the person decides.

// Nothing outside the tests calls this until the deep-link receiver lands
// (docs 23 §5, step 3). Remove this line in that change — left in, it would
// hide the next thing here that genuinely goes unused.
#![cfg_attr(not(test), allow(dead_code))]

use crate::commands::clean_header;
use percent_encoding::percent_decode_str;

/// More recipients than this across to, cc and bcc are cut, and the cut is
/// reported. A link naming five hundred people is not a mistake anybody makes
/// by clicking; a link that quietly mailed the first fifty would be worse.
pub const MAX_RECIPIENTS: usize = 50;
/// RFC 5322's line limit. A subject is one header line.
pub const MAX_SUBJECT_CHARS: usize = 998;
/// Enough for any body a person could mean to prefill.
pub const MAX_BODY_CHARS: usize = 100_000;

/// A draft, as a link asked for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MailtoDraft {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// Something was cut to the limits above. The composer says so, rather than
    /// letting a draft look like everything the link named.
    pub truncated: bool,
}

/// Decodes `%XX` and leaves `+` alone. A form submission spells a space `+`;
/// a `mailto:` spells it `%20`, and `+` is a real character in addresses such
/// as `name+tag@example.com`. Invalid UTF-8 is replaced rather than refused.
fn decode(raw: &str) -> String {
    percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

/// The addresses in one field: comma-separated, trimmed, one line each.
fn addresses(raw: &str) -> impl Iterator<Item = String> + '_ {
    raw.split(',')
        .map(|part| clean_header(decode(part).trim()))
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
}

/// A body is text with line breaks. CRLF and bare CR become LF; every other
/// control character goes except tab. Never markup: the composer puts this in
/// as text.
fn body_text(raw: &str) -> String {
    decode(raw)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// At most `limit` characters, and whether anything was cut.
fn bounded(text: String, limit: usize) -> (String, bool) {
    if text.chars().count() <= limit {
        return (text, false);
    }
    (text.chars().take(limit).collect(), true)
}

/// Parses a `mailto:` URL, or returns `None` for anything that is not one.
pub fn parse_mailto(url: &str) -> Option<MailtoDraft> {
    let rest = url
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        .map(|_| &url[7..])?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));

    let mut draft = MailtoDraft::default();
    draft.to.extend(addresses(path));
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        // Five fields and no others. `from`, `reply-to`, `in-reply-to` and the
        // rest would let a page choose who a message is from or what it
        // answers; attachments are not a thing a URL can give.
        match decode(key).to_ascii_lowercase().as_str() {
            "to" => draft.to.extend(addresses(value)),
            "cc" => draft.cc.extend(addresses(value)),
            "bcc" => draft.bcc.extend(addresses(value)),
            // The first of each, as a link has no business asking twice.
            "subject" if draft.subject.is_empty() => {
                let (text, cut) = bounded(clean_header(&decode(value)), MAX_SUBJECT_CHARS);
                draft.subject = text;
                draft.truncated |= cut;
            }
            "body" if draft.body.is_empty() => {
                let (text, cut) = bounded(body_text(value), MAX_BODY_CHARS);
                draft.body = text;
                draft.truncated |= cut;
            }
            _ => {}
        }
    }

    // The recipient cap, applied in reading order: to, then cc, then bcc.
    let mut room = MAX_RECIPIENTS;
    for list in [&mut draft.to, &mut draft.cc, &mut draft.bcc] {
        if list.len() > room {
            list.truncate(room);
            draft.truncated = true;
        }
        room -= list.len();
    }
    Some(draft)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_five_fields() {
        let d = parse_mailto(
            "mailto:sam@example.com?cc=dana@example.com&bcc=legal@example.com\
             &subject=Q3%20contracts&body=See%20attached.",
        )
        .unwrap();
        assert_eq!(d.to, ["sam@example.com"]);
        assert_eq!(d.cc, ["dana@example.com"]);
        assert_eq!(d.bcc, ["legal@example.com"]);
        assert_eq!(d.subject, "Q3 contracts");
        assert_eq!(d.body, "See attached.");
        assert!(!d.truncated);
    }

    #[test]
    fn is_case_blind_about_the_scheme_and_the_keys() {
        let d = parse_mailto("MAILTO:sam@example.com?SUBJECT=Hi&Cc=dana@example.com").unwrap();
        assert_eq!(d.subject, "Hi");
        assert_eq!(d.cc, ["dana@example.com"]);
    }

    #[test]
    fn refuses_anything_that_is_not_mailto() {
        for url in [
            "http://example.com",
            "mailto",
            "",
            "javascript:alert(1)",
            "mail:x@y",
        ] {
            assert!(parse_mailto(url).is_none(), "{url:?}");
        }
    }

    /* The reason this module exists. */
    #[test]
    fn ignores_every_field_but_the_five() {
        let d = parse_mailto(
            "mailto:sam@example.com?from=ceo@bank.example&reply-to=attacker@example.com\
             &in-reply-to=%3Cid%3E&attach=/etc/passwd&x-anything=1",
        )
        .unwrap();
        assert_eq!(
            d,
            MailtoDraft {
                to: vec!["sam@example.com".into()],
                ..Default::default()
            }
        );
    }

    #[test]
    fn a_header_cannot_carry_a_second_header() {
        // A decoded CRLF in the subject is the classic injection.
        let d = parse_mailto("mailto:sam@example.com?subject=Hi%0D%0ABcc:%20attacker@example.com")
            .unwrap();
        assert!(
            !d.subject.contains('\r') && !d.subject.contains('\n'),
            "{:?}",
            d.subject
        );
        assert_eq!(d.bcc, Vec::<String>::new());
        // And in an address.
        let d = parse_mailto("mailto:sam@example.com%0D%0ABcc:attacker@example.com").unwrap();
        assert!(d.to.iter().all(|a| !a.contains('\n') && !a.contains('\r')));
    }

    #[test]
    fn a_body_keeps_its_line_breaks_and_nothing_else() {
        let d = parse_mailto("mailto:?body=one%0D%0Atwo%0Athree%0Dfour%00%1Bfive%09tab").unwrap();
        assert_eq!(d.body, "one\ntwo\nthree\nfour".to_string() + "five\ttab");
    }

    #[test]
    fn a_plus_is_a_plus() {
        let d = parse_mailto("mailto:name+tag@example.com?subject=a+b").unwrap();
        assert_eq!(d.to, ["name+tag@example.com"]);
        assert_eq!(d.subject, "a+b");
    }

    #[test]
    fn several_addresses_and_several_to_fields_all_count() {
        let d =
            parse_mailto("mailto:a@x.example,b@x.example?to=c@x.example,%20d@x.example").unwrap();
        assert_eq!(
            d.to,
            ["a@x.example", "b@x.example", "c@x.example", "d@x.example"]
        );
    }

    #[test]
    fn caps_recipients_and_says_so() {
        let many = (0..60)
            .map(|i| format!("p{i}@x.example"))
            .collect::<Vec<_>>()
            .join(",");
        let d = parse_mailto(&format!("mailto:{many}?cc=late@x.example")).unwrap();
        assert_eq!(d.to.len(), MAX_RECIPIENTS);
        assert!(d.cc.is_empty(), "no room left once to is full");
        assert!(d.truncated);
    }

    #[test]
    fn caps_the_subject_and_the_body_and_says_so() {
        let long = "x".repeat(MAX_SUBJECT_CHARS + 10);
        let d = parse_mailto(&format!("mailto:?subject={long}")).unwrap();
        assert_eq!(d.subject.chars().count(), MAX_SUBJECT_CHARS);
        assert!(d.truncated);
    }

    #[test]
    fn takes_the_first_subject_and_body_only() {
        let d = parse_mailto("mailto:?subject=first&subject=second&body=one&body=two").unwrap();
        assert_eq!((d.subject.as_str(), d.body.as_str()), ("first", "one"));
    }

    #[test]
    fn survives_malformed_percent_sequences() {
        let d = parse_mailto("mailto:?subject=100%25%20sure%zz%E2%82").unwrap();
        assert!(d.subject.starts_with("100% sure"), "{:?}", d.subject);
    }
}
