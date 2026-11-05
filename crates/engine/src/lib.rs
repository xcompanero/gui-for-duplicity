//! # engine
//!
//! The duplicity engine: everything the GUI will ever need to know about
//! driving the `duplicity` command line lives in this crate. The UI layer
//! (Tauri shell, added in Phase 1) is a thin client of this library.
//!
//! Modules:
//! - [`profile`] — backup profile model, persisted as TOML
//! - [`collection`] — parsing of `duplicity collection-status` output
//! - [`runner`] — spawning duplicity processes and capturing their output

pub mod collection;
pub mod files;
pub mod jobs;
pub mod listing_cache;
pub mod notify;
pub mod profile;
pub mod runner;
pub mod schedule;
pub mod secrets;
pub mod verify;
