//! Parsing (mail-parser), the sanitizer profile (M1), and message building.
//!
//! Security-sensitive by definition: everything here consumes attacker-supplied
//! bytes. Two rules hold throughout — **never panic on hostile input** (a
//! malformed message must degrade, not crash the engine), and **never lose the
//! original** (parsing produces a view; the raw bytes stay the source of truth).

pub mod charset;
mod encoded_word;
pub mod legacy_cjk;
pub mod utf7;

pub mod darken;
pub mod ical;
pub mod parse;
pub mod sanitize;

pub use parse::{
    Attachment, AuthVerdict, Authentication, ParsedMessage, QuotedPicture, Unsubscribe,
    attachment_bytes, authentication, data_url, embed_cid_images, parse_message, quoted_pictures,
    unsubscribe_info,
};
pub use sanitize::{
    SanitizeReport, Sanitized, declares_dark, plain_text_to_html, resolve_cids, sanitize_html,
};
