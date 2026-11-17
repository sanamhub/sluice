//! Throwaway spike for T88: a short code meeting place beside `iroh-relay`.
//!
//! The design is written up in the PR and in ADR 0041. This crate exists only to answer three
//! questions with evidence:
//!
//! 1. can one TLS listener serve the relay and a `/meet` WebSocket on the same port;
//! 2. does `spake2` build for `wasm32-unknown-unknown` with the browser build's dependencies;
//! 3. does the full exchange work, including the burn on a wrong word pair.
//!
//! Nothing here is production code.

pub mod code;
pub mod crypto;
pub mod meet;
pub mod server;
pub mod wire;
pub mod words;
