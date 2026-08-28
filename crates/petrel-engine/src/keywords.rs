//! Tags as IMAP keywords, for servers that are not Gmail.
//!
//! Gmail carries a tag as a label; everywhere else the protocol's own
//! vehicle is a message keyword — an atom stored beside \Seen and \Flagged,
//! which Dovecot persists (`PERMANENTFLAGS (... \*)`) and other clients
//! display. Atoms are narrow: no spaces, no control characters, none of
//! `(){%*"\]`, ASCII only. A tag named "Waiting on" travels as
//! `Waiting_on` — lossy but stable, and stable is what matters, because
//! the same tag must munge the same way on every delivery.

/// The keyword atom a tag travels as.
///
/// Every character an atom cannot carry becomes `_`. Two tags that munge
/// alike ("Waiting on", "Waiting_on") share a keyword; the rename that
/// avoids it is cheaper than an escaping scheme no other client reads.
pub fn tag_keyword(name: &str) -> String {
    let munged: String = name
        .chars()
        .map(|c| {
            let atom_safe = c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '=');
            if atom_safe { c } else { '_' }
        })
        .collect();
    // An empty or all-munged name still needs to be a valid atom.
    if munged.chars().all(|c| c == '_') {
        format!("tag{}", munged.len())
    } else {
        munged
    }
}

/// Whether a keyword is the machine's rather than a person's.
///
/// RFC 5788 reserves the `$` prefix for registered keywords — `$Forwarded`,
/// `$MDNSent`, `$Junk`, `$NotJunk`, `$Phishing` — and clients help themselves
/// to the same space for their own bookkeeping: Apple Mail's message
/// categories arrive as things like `$purchases`. `\` is the IMAP system flag
/// prefix and never belongs to a user either.
///
/// These still sync as flags, because round-tripping them is correct. What
/// they must not do is appear in the sidebar as though somebody made them.
/// A tag list that fills up with `$purchases` and `$MDNSent` is a list nobody
/// wants, and the first one arrived without anyone asking.
///
/// A tag the person made themselves is matched by munge before this is
/// consulted, so naming a tag "$thing" still works: it travels as `_thing`
/// and comes back to the tag it left from.
pub fn is_system_keyword(keyword: &str) -> bool {
    keyword.starts_with('$') || keyword.starts_with('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoms_survive_and_specials_munge() {
        assert_eq!(tag_keyword("Urgent"), "Urgent");
        assert_eq!(tag_keyword("Waiting on"), "Waiting_on");
        assert_eq!(tag_keyword("Q3 (review)"), "Q3__review_");
        // A name with no atom-safe character at all still yields a valid,
        // stable atom rather than a row of underscores.
        assert_eq!(tag_keyword("日本語"), "tag3");
        assert_eq!(tag_keyword("日本語"), tag_keyword("日本語"), "stable");
    }

    #[test]
    fn machine_keywords_are_not_somebody_s_tags() {
        // RFC 5788's registered ones, and the kind clients invent. $purchases
        // is a real example: it arrived on a live account from another client
        // and turned into a sidebar entry nobody had made.
        for kw in [
            "$Forwarded",
            "$MDNSent",
            "$Junk",
            "$NotJunk",
            "$Phishing",
            "$purchases",
        ] {
            assert!(is_system_keyword(kw), "{kw} should be the machine's");
        }
        for kw in ["\\Seen", "\\Flagged", "\\Draft"] {
            assert!(is_system_keyword(kw), "{kw} should be the machine's");
        }
    }

    #[test]
    fn a_persons_tags_are_left_alone() {
        for kw in [
            "Urgent",
            "Waiting_on",
            "Receipts",
            "_thing",
            "2026",
            "a.b-c+d=e",
        ] {
            assert!(!is_system_keyword(kw), "{kw} is somebody's tag");
        }
    }

    #[test]
    fn a_tag_named_with_a_dollar_still_round_trips() {
        // It travels as an atom without the $, so it comes back matched by
        // munge to the tag it left from, and never looks like a system one.
        let travelled = tag_keyword("$mine");
        assert_eq!(travelled, "_mine");
        assert!(!is_system_keyword(&travelled));
    }
}
