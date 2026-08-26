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
}
