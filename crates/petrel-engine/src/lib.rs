//! Petrel's headless mail engine.
//!
//! Owns everything trusted: storage, full-text search, the action journal, and
//! (as the crate grows) sync orchestration and sanitization. UI shells talk to
//! this crate through a typed API; they never touch SQLite or the network.

pub mod blob;
pub mod outbox;
pub mod retention;
pub mod store;
