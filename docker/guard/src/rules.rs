//! The checks of ADR 0019 and ADR 0020 on a config's text: it parses with every key known, every
//! setting that would open something by default is written out, and the config is what its mode
//! says. A closed relay has a closed `access`, an open one has `--open` and a per-client byte
//! limit, and a rendezvous server has its relay half off.
//!
//! No message prints a value from the config. A `shared_token` entry or `access.http.bearer_token`
//! is a secret, and so may be the query of `access.http.url`, and a misplaced one would turn up in
//! the error of whatever key it was put under. toml's own messages quote values, so they are read
//! for the kind of error and the line, and never passed on.

mod access;
mod messages;

use crate::schema::{AccessConfig, Config};
use crate::{Mode, Refusal};

/// The whole check of a config's text.
pub(crate) fn check(
    text: &str,
    mode: Mode,
    access_token: Option<&str>,
    behind_proxy: bool,
) -> Result<(), Refusal> {
    let readme = readme(mode);
    // iroh-relay 1.4.1 stops at start on an empty token: an empty value replaces the list with one
    // empty entry, and `bail_any!` refuses it (main.rs:236-243). The guard refuses it first so the
    // operator gets exit 64 and one line that names the variable.
    if access_token == Some("") {
        return Err(Refusal::new(
            "IROH_RELAY_ACCESS_TOKEN is set but empty: unset it, or set it to the token clients \
             present",
        ));
    }
    // A syntax error first, so the message can say the file is not TOML at all.
    if let Err(err) = text.parse::<toml::Table>() {
        return Err(Refusal::new(format!(
            "the config is not valid TOML{}: check it against {readme}",
            messages::at_line(text, &err)
        )));
    }
    let config: Config =
        toml::from_str(text).map_err(|err| messages::schema_error(text, &err, readme))?;
    match mode {
        Mode::Rendezvous => check_rendezvous(&config)?,
        Mode::Closed | Mode::Open => check_relay_half_on(&config)?,
    }
    if let Some(access) = &config.access {
        access::check(access, access_token)?;
    }
    check_tls(&config, mode, behind_proxy)?;
    check_metrics(&config)?;
    match mode {
        Mode::Rendezvous => Ok(()),
        Mode::Closed | Mode::Open => check_access(&config, mode),
    }
}

/// What the operator is told to compare a config with: the README of the image that mode runs.
fn readme(mode: Mode) -> &'static str {
    match mode {
        Mode::Rendezvous => "docker/rendezvous/README.md",
        Mode::Closed | Mode::Open => "docker/relay/README.md",
    }
}

/// A rendezvous server answers address discovery and relays nothing (ADR 0013). `enable_relay`
/// defaults to true (main.rs:107 and 376-378), so it must say false, and a deleted line cannot turn
/// the server into a relay. `access` and `[limits]` govern only the relay half, so a config that
/// sets them was written for a relay.
fn check_rendezvous(config: &Config) -> Result<(), Refusal> {
    if config.enable_relay != Some(false) {
        return Err(Refusal::new(
            "--rendezvous needs `enable_relay = false` written out: iroh-relay 1.4.1 turns the \
             relay half on without it, and a rendezvous server relays nothing",
        ));
    }
    if config.access.is_some() {
        return Err(Refusal::new(
            "`access` is set, and a rendezvous server relays nothing for it to govern: remove \
             it, or start a relay without --rendezvous",
        ));
    }
    if config.limits.is_some() {
        return Err(Refusal::new(
            "[limits] is set, and a rendezvous server relays nothing for it to limit: remove \
             it, or start a relay without --rendezvous",
        ));
    }
    Ok(())
}

/// A config with the relay half off is a rendezvous config, and starts only as one.
fn check_relay_half_on(config: &Config) -> Result<(), Refusal> {
    if config.enable_relay == Some(false) {
        return Err(Refusal::new(
            "`enable_relay` is false, so this is a rendezvous config: start it with --rendezvous, \
             as the rendezvous image does",
        ));
    }
    Ok(())
}

/// HTTPS stays on, and a relay with no `[tls]` at all needs `--behind-proxy`. `dangerous_http_only`
/// turns HTTPS off on a relay that has TLS (main.rs:593-600 and 703-721), which is what `--dev`
/// does, so a shared token would then cross the network in clear text. A config with no `[tls]`
/// serves plain HTTP only, which is right behind a proxy that ends TLS and wrong anywhere else: the
/// guard cannot tell those apart, so the operator says which one it is with the flag. A rendezvous
/// server relays nothing and answers no token, so the flag has nothing to stand behind there.
fn check_tls(config: &Config, mode: Mode, behind_proxy: bool) -> Result<(), Refusal> {
    if config
        .tls
        .as_ref()
        .is_some_and(|tls| tls.dangerous_http_only)
    {
        return Err(Refusal::new(
            "`dangerous_http_only` is true, which turns HTTPS off as --dev does: remove it, or \
             leave out [tls] and start with --behind-proxy behind a proxy that ends TLS",
        ));
    }
    match mode {
        Mode::Rendezvous => match behind_proxy {
            true => Err(Refusal::new(
                "--behind-proxy is for a relay behind a proxy that ends TLS, and a rendezvous \
                 server relays nothing: remove it, or start a relay without --rendezvous",
            )),
            false => Ok(()),
        },
        Mode::Closed | Mode::Open => match (config.tls.is_none(), behind_proxy) {
            (true, false) => Err(Refusal::new(
                "there is no [tls] table, so iroh-relay serves plain HTTP only, and a shared token \
                 or an HTTP bearer token would cross the network in clear text: add [tls], or \
                 start with --behind-proxy when a proxy in front ends TLS",
            )),
            (false, true) => Err(Refusal::new(
                "--behind-proxy was given, but the config has a [tls] table, so iroh-relay ends \
                 TLS itself: remove the flag, or leave out [tls] when a proxy in front ends TLS",
            )),
            _ => Ok(()),
        },
    }
}

/// iroh-relay runs its metrics server unless the file says otherwise (main.rs:145 and 384-386),
/// on port 9090 with no login. Either value passes, as long as someone wrote it.
fn check_metrics(config: &Config) -> Result<(), Refusal> {
    match config.enable_metrics {
        Some(_) => Ok(()),
        None => Err(Refusal::new(
            "`enable_metrics` is not set, and iroh-relay 1.4.1 then serves metrics on port 9090 \
             with no login: write `enable_metrics = false`, or `true` to keep them on purpose",
        )),
    }
}

/// A relay's `access` is closed without `--open`, and open with it and a byte limit.
fn check_access(config: &Config, mode: Mode) -> Result<(), Refusal> {
    let name = access_name(config.access.as_ref());
    match (is_closed(config), mode) {
        (true, Mode::Open) => Err(Refusal::new(format!(
            "--open was given, but `access` is {name}, which is closed: remove --open, or point \
             --config-path at an open config such as /etc/relay/open.toml"
        ))),
        (false, Mode::Closed) => Err(Refusal::new(format!(
            "`access` is {name}: list the ids that may relay in `[access] allowlist`, or start \
             with --open to run it open on purpose"
        ))),
        (false, _) => check_rate(config),
        (true, _) => Ok(()),
    }
}

/// Closed is an allowlist, an HTTP check or shared tokens. Everything else lets in ids nobody
/// listed: everyone, a denylist, and no `access` at all, which iroh-relay reads as everyone.
fn is_closed(config: &Config) -> bool {
    matches!(
        config.access,
        Some(AccessConfig::Allowlist(_) | AccessConfig::Http(_) | AccessConfig::SharedToken(_))
    )
}

fn access_name(access: Option<&AccessConfig>) -> &'static str {
    match access {
        None => "not set, which iroh-relay 1.4.1 reads as everyone",
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
