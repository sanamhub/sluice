//! The checks of ADR 0019 and ADR 0020 on a config's text: it parses with every key known, every
//! setting that would open something by default is written out, and the relay is closed, or open
//! with `--open` and a per-client byte limit.
//!
//! No message prints a value from the config. A `shared_token` entry or `access.http.bearer_token`
//! is a secret, and so may be the query of `access.http.url`, and a misplaced one would turn up in
//! the error of whatever key it was put under. toml's own messages quote values, so they are read
//! for the kind of error and the line, and never passed on.

mod access;
mod messages;

use crate::schema::{AccessConfig, Config};
use crate::{Mode, Refusal};

/// What the operator is told to compare a config with.
const README: &str = "docker/relay/README.md";

/// The whole check of a config's text.
pub(crate) fn check(text: &str, open: bool) -> Result<Mode, Refusal> {
    // A syntax error first, so the message can say the file is not TOML at all.
    if let Err(err) = text.parse::<toml::Table>() {
        return Err(Refusal::new(format!(
            "the config is not valid TOML{}: check it against {README}",
            messages::at_line(text, &err)
        )));
    }
    let config: Config = toml::from_str(text).map_err(|err| messages::schema_error(text, &err))?;
    if let Some(access) = &config.access {
        access::check(access)?;
    }
    check_tls(&config)?;
    check_metrics(&config)?;
    let mode = mode_of(&config);
    match (mode, open) {
        (Mode::Closed, false) => Ok(Mode::Closed),
        (Mode::Closed, true) => Err(Refusal::new(format!(
            "--open was given, but `access` is {}, which is closed: remove --open, or point \
             --config-path at an open config such as /etc/relay/open.toml",
            access_name(config.access.as_ref())
        ))),
        (Mode::Open, false) => Err(Refusal::new(format!(
            "`access` is {}: list the ids that may relay in `[access] allowlist`, or start with \
             --open to run it open on purpose",
            access_name(config.access.as_ref())
        ))),
        (Mode::Open, true) => check_rate(&config).map(|()| Mode::Open),
    }
}

/// `dangerous_http_only = true` turns HTTPS off, which is what `--dev` does (main.rs:593-600 and
/// 703-721), and a shared token would then cross the network in clear text. No `[tls]` at all
/// stays allowed, for a relay behind a proxy that ends TLS.
fn check_tls(config: &Config) -> Result<(), Refusal> {
    if config
        .tls
        .as_ref()
        .is_some_and(|tls| tls.dangerous_http_only)
    {
        return Err(Refusal::new(
            "`dangerous_http_only` is true, which turns HTTPS off as --dev does: remove it, or \
             leave out [tls] for a relay behind a proxy that ends TLS",
        ));
    }
    Ok(())
}

/// iroh-relay runs its metrics server unless the file says otherwise (main.rs:145 and 384-386),
/// on port 9090 with no login. Either value passes, as long as someone wrote it.
fn check_metrics(config: &Config) -> Result<(), Refusal> {
    match config.enable_metrics {
        Some(_) => Ok(()),
        None => Err(Refusal::new(
            "`enable_metrics` is not set, and iroh-relay 1.4.0 then serves metrics on port 9090 \
             with no login: write `enable_metrics = false`, or `true` to keep them on purpose",
        )),
    }
}

/// Closed is an allowlist, an HTTP check or shared tokens. Everything else lets in ids nobody
/// listed: everyone, a denylist, and no `access` at all, which iroh-relay reads as everyone.
fn mode_of(config: &Config) -> Mode {
    match config.access {
        Some(AccessConfig::Allowlist(_) | AccessConfig::Http(_) | AccessConfig::SharedToken(_)) => {
            Mode::Closed
        }
        Some(AccessConfig::Everyone | AccessConfig::Denylist(_)) | None => Mode::Open,
    }
}

fn access_name(access: Option<&AccessConfig>) -> &'static str {
    match access {
        None => "not set, which iroh-relay 1.4.0 reads as everyone",
        Some(AccessConfig::Everyone) => "\"everyone\", which lets anyone relay",
        Some(AccessConfig::Allowlist(_)) => "an allowlist",
        Some(AccessConfig::Denylist(_)) => "a denylist, which lets in every id not on it",
        Some(AccessConfig::Http(_)) => "an HTTP check",
        Some(AccessConfig::SharedToken(_)) => "a list of shared tokens",
    }
}

/// An open relay needs the per-client byte limit of ADR 0011. iroh-relay refuses a limit of zero
/// itself, at start, so the guard only asks that one is set.
fn check_rate(config: &Config) -> Result<(), Refusal> {
    let rate = config
        .limits
        .as_ref()
        .and_then(|limits| limits.client.as_ref())
        .and_then(|client| client.rx.as_ref())
        .and_then(|rx| rx.bytes_per_second);
    match rate {
        Some(_) => Ok(()),
        None => Err(Refusal::new(
            "--open needs `limits.client.rx.bytes_per_second`, the most each client may send into \
             the relay: set it under [limits.client.rx], as /etc/relay/open.toml does",
        )),
    }
}
