//! `sluice-relay-guard`: the entry point of the `relay` image (ADR 0019). It checks the config that
//! `iroh-relay` is about to start with, and refuses one that would open the relay by mistake.
//!
//! iroh-relay 1.4.0 reads a missing `access` setting as everyone, ignores keys it does not know,
//! and runs its open default when the config file is missing. Each gives a relay that anyone can
//! use with no byte limit, and the server logs nothing that says so. The guard refuses all three,
//! and every config that is open without `--open` and a per-client byte limit.
//!
//! It owns the check and the start, and nothing else. It never prints a value from the config,
//! since a token may be one, and it never changes the config it was given.

use std::fmt;

mod args;
mod file;
mod rules;
mod schema;

pub use args::Args;
pub use file::{CONFIG_ON_STDIN, open_config, relay_args};

/// The iroh-relay release whose config `schema.rs` mirrors. `docker/relay/Dockerfile` must build
/// this one, and the root `Cargo.lock` must resolve it, or a test fails: a new release may add a
/// key the guard does not know, so its schema has to be read again first.
///
/// ```
/// assert_eq!(sluice_relay_guard::IROH_RELAY_VERSION, "1.4.0");
/// ```
pub const IROH_RELAY_VERSION: &str = "1.4.0";

/// Where the image keeps the relay the guard starts.
///
/// ```
/// assert!(sluice_relay_guard::IROH_RELAY.ends_with("/iroh-relay"));
/// ```
pub const IROH_RELAY: &str = "/usr/local/bin/iroh-relay";

/// The exit code of every refusal: `EX_USAGE` of BSD's sysexits.h, a usage error.
///
/// ```
/// assert_eq!(sluice_relay_guard::EXIT_REFUSED, 64);
/// ```
pub const EXIT_REFUSED: u8 = 64;

/// Whether a config lets in only the ids someone chose, or anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `access` is an allowlist, an HTTP check or shared tokens.
    Closed,
    /// `access` is everyone, a denylist, or not set at all.
    Open,
}

/// Why the guard will not start the relay: one line that names the key or the rule, and says what
/// to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(String);

impl Refusal {
    fn new(message: impl Into<String>) -> Refusal {
        Refusal(message.into())
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// Checks a config's text: every key is one iroh-relay 1.4.0 knows, every allowlist id is 64 hex
/// digits, `enable_metrics` is written out, HTTPS is not turned off, an HTTP access URL holds no
/// secret, and the relay is closed, or `open` is set and so is a per-client byte limit. `--open`
/// with a closed config is refused too, so the flag always means what it says.
///
/// ```
/// use sluice_relay_guard::{check_config, Mode};
///
/// let closed = "enable_metrics = false\n[access]\nallowlist = []\n";
/// assert_eq!(check_config(closed, false), Ok(Mode::Closed));
/// assert!(check_config("enable_metrics = false\n[acess]\nallowlist = []\n", false).is_err());
/// assert!(check_config("[access]\nallowlist = []\n", false).is_err());
/// assert!(check_config("enable_metrics = false\naccess = \"everyone\"\n", false).is_err());
///
/// let open = "enable_metrics = false\naccess = \"everyone\"\n\
///             [limits.client.rx]\nbytes_per_second = 2097152\n";
/// assert_eq!(check_config(open, true), Ok(Mode::Open));
/// ```
pub fn check_config(text: &str, open: bool) -> Result<Mode, Refusal> {
    rules::check(text, open)
}

#[cfg(test)]
mod tests;
