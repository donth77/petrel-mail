//! Parsing (mail-parser), the sanitizer profile (M1), and message building.
//!
//! Security-sensitive by definition: everything here consumes attacker-supplied
//! bytes. Two rules hold throughout — **never panic on hostile input** (a
//! malformed message must degrade, not crash the engine), and **never lose the
//! original** (parsing produces a view; the raw bytes stay the source of truth).

pub mod parse;

pub use parse::{Attachment, ParsedMessage, parse_message};
