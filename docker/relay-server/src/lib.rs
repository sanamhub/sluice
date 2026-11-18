//! The relay image's server: `iroh-relay`'s `/relay` and the short-code `/meet` on one listener.
//!
//! The guard has checked the config before this crate reads it, but it reads the same schema, and
//! refuses an unknown key too. `server::start` is the part a test drives; `main.rs` adds the
//! command-line path and the signal handling.

pub mod access;
pub mod config;
pub mod meet;
pub mod server;
