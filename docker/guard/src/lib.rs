//! `sluice-relay-guard`: the entry point of the `relay` and `rendezvous` images (ADR 0019, ADR
//! 0020). It checks the config that `iroh-relay` is about to start with, and refuses one that would
//! open more than the operator wrote.
//!
//! iroh-relay 1.4.0 reads a missing `access` setting as everyone, ignores keys it does not know,
//! and runs its open default when the config file is missing. Each gives a relay that anyone can
//! use with no byte limit, and the server logs nothing that says so. The guard refuses all three,
//! and every config that is open without `--open` and a per-client byte limit. In `--rendezvous`
//! mode it requires the relay half off, since a deleted `enable_relay = false` line would turn a
//! rendezvous server into an open relay.
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

/// The iroh-relay release whose config `schema.rs` mirrors. Both Dockerfiles must build this one,
/// and the root `Cargo.lock` must resolve it, or a test fails: a new release may add a key the
/// guard does not know, so its schema has to be read again first.
///
/// ```
/// assert_eq!(sluice_relay_guard::IROH_RELAY_VERSION, "1.4.0");
/// ```
pub const IROH_RELAY_VERSION: &str = "1.4.0";

/// Where both images keep the relay the guard starts.
///
/// ```
/// assert!(sluice_relay_guard::IROH_RELAY.ends_with("/iroh-relay"));
/// ```
pub const IROH_RELAY: &str = "/usr/local/bin/iroh-relay";

/// The environment variable iroh-relay 1.4.0 replaces the whole `access.shared_token` list with,
/// if it is set and is UTF-8 (main.rs:43 and 236-241). The guard reads it the same way, and passes
/// it to [`check_config`].
///
/// ```
/// assert_eq!(sluice_relay_guard::ACCESS_TOKEN_VAR, "IROH_RELAY_ACCESS_TOKEN");
/// ```
pub const ACCESS_TOKEN_VAR: &str = "IROH_RELAY_ACCESS_TOKEN";

/// The exit code of every refusal: `EX_USAGE` of BSD's sysexits.h, a usage error.
///
/// ```
/// assert_eq!(sluice_relay_guard::EXIT_REFUSED, 64);
/// ```
pub const EXIT_REFUSED: u8 = 64;

/// What the operator asked the guard to start, by the flag they passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No flag: a relay whose `access` is an allowlist, an HTTP check or shared tokens.
    Closed,
    /// `--open`: a relay whose `access` is everyone, a denylist, or not set, with a per-client
    /// byte limit.
    Open,
    /// `--rendezvous`: address discovery only, with the relay half off.
    Rendezvous,
}

/// Why the guard will not start the relay: one line that names the key or the rule, and says what
/// to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(String);

impl Refusal {
    /// Every control character is escaped, so a refusal is one line in any log, whatever it
    /// quotes: a path, or a key name from the config.
    fn new(message: impl Into<String>) -> Refusal {
        Refusal(escape_controls(&message.into()))
    }
}

/// `text` with each control character, a newline or an escape code among them, written as a Rust
/// escape such as `\n` or `\u{1b}`.
fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c.is_control() {
            true => out.extend(c.escape_default()),
            false => out.push(c),
        }
    }
    out
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// Checks a config's text for the mode asked for. In every mode: every key is one iroh-relay 1.4.0
/// knows, every allowlist id is one iroh-relay accepts, `enable_metrics` is written out, HTTPS is
/// not turned off, an HTTP access URL holds no secret, and shared tokens are not empty. Closed
/// needs a closed `access`, open needs an open one and a per-client byte limit, and rendezvous
/// needs `enable_relay = false` and no `access` or `[limits]`. Each flag is refused with a config
/// of another mode, so it always means what it says.
///
/// `access_token` is the value of [`ACCESS_TOKEN_VAR`], or `None` if it is unset or not UTF-8, as
/// iroh-relay reads it. A value that is set replaces the config's `shared_token` list, and an
/// empty one is refused.
///
/// ```
/// use sluice_relay_guard::{check_config, Mode};
///
/// let closed = "enable_metrics = false\n[access]\nallowlist = []\n";
/// assert_eq!(check_config(closed, Mode::Closed, None), Ok(()));
/// assert!(check_config(closed, Mode::Open, None).is_err());
/// let typo = "enable_metrics = false\n[acess]\nallowlist = []\n";
/// assert!(check_config(typo, Mode::Closed, None).is_err());
/// assert!(check_config("[access]\nallowlist = []\n", Mode::Closed, None).is_err());
///
/// let open = "enable_metrics = false\naccess = \"everyone\"\n\
///             [limits.client.rx]\nbytes_per_second = 2097152\n";
/// assert_eq!(check_config(open, Mode::Open, None), Ok(()));
/// assert!(check_config(open, Mode::Closed, None).is_err());
///
/// let rendezvous = "enable_relay = false\nenable_metrics = false\n";
/// assert_eq!(check_config(rendezvous, Mode::Rendezvous, None), Ok(()));
/// assert!(check_config("enable_metrics = false\n", Mode::Rendezvous, None).is_err());
///
/// // The variable replaces an empty list, and is refused when it is empty itself.
/// let tokens = "enable_metrics = false\n[access]\nshared_token = []\n";
/// assert!(check_config(tokens, Mode::Closed, None).is_err());
/// assert_eq!(check_config(tokens, Mode::Closed, Some("t")), Ok(()));
/// assert!(check_config(closed, Mode::Closed, Some("")).is_err());
/// ```
pub fn check_config(text: &str, mode: Mode, access_token: Option<&str>) -> Result<(), Refusal> {
    rules::check(text, mode, access_token)
}

#[cfg(test)]
mod tests;
