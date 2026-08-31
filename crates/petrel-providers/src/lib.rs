//! Provider backends behind one trait: capability-adaptive IMAP/SMTP, the Gmail
//! API, and Microsoft Graph (JMAP later). Engine logic never sees provider
//! semantics directly.
//!
//! M0 status: the IMAP slice (connect, capabilities, LIST, SELECT, FETCH,
//! APPEND) is in place; the common backend trait lands once a second backend
//! exists to generalise against — inventing it from one implementation is how
//! provider semantics leak.

pub mod imap;
pub mod oauth;
pub mod smtp;
