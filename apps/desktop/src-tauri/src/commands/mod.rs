//! The IPC surface, one file per area of the UI that calls it.

pub(crate) mod accounts;
pub(crate) mod attachments;
pub(crate) mod compose;
pub(crate) mod files;
pub(crate) mod invitations;
pub(crate) mod mail;
pub(crate) mod outbox;
pub(crate) mod remote;
pub(crate) mod settings;
pub(crate) mod storage;
pub(crate) mod triage;
pub(crate) mod updates;
pub(crate) mod windows;

/// Strips the characters that would end a header line and start another one.
///
/// A mail header is a line, so a value carrying CR or LF is not a value: it
/// is however many headers the sender felt like writing. The way in is
/// ordinary mail — a subject encoded as `=?utf-8?q?Hi=0D=0AReply-To:...?=`
/// decodes to a string with a CRLF in it, the composer prefixes `Re: `, and
/// the reply goes out carrying an attacker's `Reply-To:` or, with two of
/// them, an attacker's body. The providers crate scrubs what it renders;
/// this is the same rule applied at the door, so a draft is never *stored*
/// carrying one.
///
/// Every C0 control goes, not only the two: a NUL or an ESC in a header is
/// no more legitimate and some agents resynchronise on them. Tabs survive
/// because folding uses them.
pub(crate) fn clean_header(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect()
}

#[cfg(test)]
mod header_tests {
    use super::clean_header;

    /// A decoded RFC 2047 word carrying a CRLF, as a crafted message delivers it.
    #[test]
    fn a_header_value_cannot_carry_a_second_header() {
        let hostile = "Invoice\r\nBcc: attacker@example.com";
        let cleaned = clean_header(hostile);
        assert!(!cleaned.contains('\r'), "{cleaned:?}");
        assert!(!cleaned.contains('\n'), "{cleaned:?}");
        assert_eq!(cleaned, "InvoiceBcc: attacker@example.com");

        // A bare LF is what an unescaped iCalendar SUMMARY becomes.
        assert_eq!(
            clean_header("Standup\nX-Injected: yes"),
            "StandupX-Injected: yes"
        );
        // And the other controls, which are no more legitimate in a header.
        assert_eq!(clean_header("a\u{0}b\u{1b}c"), "abc");
    }

    /// Ordinary subjects are left exactly as they are — including the tab
    /// that header folding is spelled with, and every non-ASCII language.
    #[test]
    fn ordinary_values_are_untouched() {
        for value in [
            "Re: Q3 invoice",
            "会議の件",
            "Fwd:\tnotes",
            "Name <someone@example.com>",
            "",
        ] {
            assert_eq!(clean_header(value), value, "{value:?}");
        }
    }
}
