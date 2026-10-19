//! The segment log for files that are still being written.
//!
//! Owns sealing segments, the `prev_entry_hash` chain, signature checks, the provisional tail and
//! the switch to a static root on close. The crate is sans-IO: bytes and timer ticks go in,
//! actions come out, so the browser build can use it unchanged. Specified in `SPEC.md` section 6.
