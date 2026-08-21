//! Conversation threading.
//!
//! Two signals, in order of trust:
//!
//! 1. **The reference graph** (`References` / `In-Reply-To`). Reliable, because
//!    Message-IDs are machine-generated and clients propagate them. Threads are
//!    unioned: a message arriving late can link two chains that were separate,
//!    which is normal when the middle of a conversation syncs after its ends.
//! 2. **Normalized subject**, as a *narrow* fallback for mail whose references
//!    were stripped by a mailing list or a broken client.
//!
//! The subject fallback is deliberately timid. Merging "Hi" from two strangers
//! into one conversation is worse than showing two threads: a wrong merge hides
//! mail inside an unrelated conversation, where the user will not look for it.
//! So it applies only to distinctive subjects, within a time window, and never
//! across accounts.

/// Subjects too generic to identify a conversation. Threading two of these
/// together would be a coin flip.
const GENERIC_SUBJECTS: &[&str] = &[
    "",
    "hi",
    "hello",
    "hey",
    "thanks",
    "thank you",
    "question",
    "update",
    "fyi",
    "info",
    "meeting",
    "reminder",
    "invitation",
    "no subject",
    "re",
    "fwd",
    "test",
    "hello there",
];

/// Reply/forward prefixes across the locales that actually appear in mail.
/// Recipients' clients localize these, so a thread routinely mixes several.
const PREFIXES: &[&str] = &[
    "re", "aw", "sv", "vs", "antw", "odp", "ref", "res", "rif", "fwd", "fw", "wg", "tr", "vb",
    "enc", "rv", "doorst", "转发", "回复", "答复",
];

/// Strips reply/forward prefixes and collapses whitespace.
///
/// Handles the real shapes: `Re:`, `RE :`, `Fwd:`, `[list] Re:`, and the
/// stacked `Re: Re: Fwd: Re:` that a long thread accumulates. The result is
/// what two messages in one conversation should agree on.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim().to_string();

    loop {
        let before = s.clone();

        // Leading list tags: "[rust-dev] Re: thing"
        if s.starts_with('[')
            && let Some(end) = s.find(']')
        {
            let rest = s[end + 1..].trim_start();
            // Only strip if something remains — "[ANN]" alone is the subject.
            if !rest.is_empty() {
                s = rest.to_string();
            }
        }

        let lower = s.to_lowercase();
        for p in PREFIXES {
            // `Re:` and the numbered `Re[2]:` variant Outlook emits.
            for candidate in [format!("{p}:"), format!("{p} :")] {
                if lower.starts_with(&candidate) {
                    s = s[candidate.len()..].trim_start().to_string();
                    break;
                }
            }
            let bracketed = format!("{p}[");
            if lower.starts_with(&bracketed)
                && let Some(close) = s.find(']')
                && s[close + 1..].starts_with(':')
            {
                s = s[close + 2..].trim_start().to_string();
            }
        }

        if s == before {
            break;
        }
    }

    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a normalized subject is distinctive enough to thread on alone.
pub fn subject_is_threadable(normalized: &str) -> bool {
    let lower = normalized.to_lowercase();
    if GENERIC_SUBJECTS.contains(&lower.as_str()) {
        return false;
    }
    // Short subjects collide too easily across unrelated mail.
    normalized.chars().count() >= 8
}

/// How far apart two messages can be and still be joined by subject alone.
pub const SUBJECT_THREAD_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_reply_and_forward_prefixes() {
        for (input, expected) in [
            ("Re: Q3 vendor contracts", "Q3 vendor contracts"),
            ("RE: Q3 vendor contracts", "Q3 vendor contracts"),
            ("Fwd: Q3 vendor contracts", "Q3 vendor contracts"),
            ("Re: Re: Fwd: Q3 vendor contracts", "Q3 vendor contracts"),
            ("Q3 vendor contracts", "Q3 vendor contracts"),
        ] {
            assert_eq!(normalize_subject(input), expected, "input {input:?}");
        }
    }

    #[test]
    fn handles_localized_prefixes() {
        // A thread crossing locales mixes these; they must all normalize to the
        // same string or the conversation splits in half.
        for input in [
            "AW: Quarterly figures",  // German
            "SV: Quarterly figures",  // Swedish/Danish
            "VS: Quarterly figures",  // Finnish
            "Odp: Quarterly figures", // Polish
            "R: Quarterly figures",   // (unknown prefix — left alone)
        ] {
            let out = normalize_subject(input);
            assert!(
                out == "Quarterly figures" || out == "R: Quarterly figures",
                "unexpected {out:?} from {input:?}"
            );
        }
        assert_eq!(
            normalize_subject("AW: Quarterly figures"),
            "Quarterly figures"
        );
    }

    #[test]
    fn strips_mailing_list_tags() {
        assert_eq!(
            normalize_subject("[rust-dev] Re: async traits"),
            "async traits"
        );
        // A tag that is the whole subject is content, not decoration.
        assert_eq!(normalize_subject("[ANN]"), "[ANN]");
    }

    #[test]
    fn collapses_whitespace_and_is_idempotent() {
        let once = normalize_subject("Re:   Q3    vendor   contracts ");
        assert_eq!(once, "Q3 vendor contracts");
        assert_eq!(
            normalize_subject(&once),
            once,
            "normalizing twice changes nothing"
        );
    }

    #[test]
    fn generic_subjects_are_not_threadable() {
        for s in ["Hi", "hello", "thanks", "FYI", "meeting", "", "Update"] {
            assert!(
                !subject_is_threadable(&normalize_subject(s)),
                "{s:?} is too generic to thread on"
            );
        }
    }

    #[test]
    fn distinctive_subjects_are_threadable() {
        for s in [
            "Q3 vendor contracts",
            "Re: async traits in 2026",
            "Invoice 2026-0912 for review",
        ] {
            assert!(subject_is_threadable(&normalize_subject(s)), "{s:?}");
        }
    }
}
