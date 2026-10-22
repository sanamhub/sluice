//! The checks of ADR 0019 on a config's text: it parses with every key known, and it is closed, or
//! open with `--open` and a per-client byte limit.
//!
//! No message prints a value from the config. A `shared_token` entry or `access.http.bearer_token`
//! is a secret, and so may be the query of `access.http.url`, and a misplaced one would turn up in
//! the error of whatever key it was put under. toml's own messages quote values, so they are read
//! for the kind of error and the line, and never passed on.

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
            at_line(text, &err)
        )));
    }
    let config: Config = toml::from_str(text).map_err(|err| schema_error(text, &err))?;
    if let Some(AccessConfig::Allowlist(ids)) = &config.access {
        check_ids(ids)?;
    }
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

/// An id is what `sluice id` prints: 64 hex digits. iroh-relay may take other spellings, and the
/// README tells the operator to paste that one. An empty list lets nobody in, and is the default.
fn check_ids(ids: &[String]) -> Result<(), Refusal> {
    for (n, id) in ids.iter().enumerate() {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            // By its place, not its text: a token pasted into the wrong list stays unprinted.
            return Err(Refusal::new(format!(
                "entry {} of `access.allowlist` is not 64 hex digits: paste the id that \
                 `sluice id` prints",
                n + 1
            )));
        }
    }
    Ok(())
}

/// The refusal for text that is TOML but not iroh-relay 1.4.0's config. It names the line, and the
/// key on it when the line has one, but never what toml quoted from the value.
fn schema_error(text: &str, err: &toml::de::Error) -> Refusal {
    let at = at(text, err);
    let message = err.message();
    if let Some(name) = quoted_after(message, "unknown field `") {
        return Refusal::new(format!(
            "{at}unknown key `{name}`: iroh-relay 1.4.0 has no such setting, and would ignore \
             it; check the spelling against {README}"
        ));
    }
    if let Some(name) = quoted_after(message, "missing field `") {
        return Refusal::new(format!(
            "{at}this table needs the key `{name}`: see {README}"
        ));
    }
    let key = key_on_line(text, err).map_or(String::new(), |key| format!("`{key}` "));
    if message.starts_with("unknown variant `") {
        return Refusal::new(format!(
            "{at}{key}is not a setting or value iroh-relay 1.4.0 knows here; check the \
             spelling against {README} (the value is not printed, since it may be a token)"
        ));
    }
    Refusal::new(format!(
        "{at}the value of {key}does not have the type iroh-relay 1.4.0 reads; check it against \
         {README} (the value is not printed, since it may be a token)"
    ))
}

/// The line an error points at, if it points anywhere.
fn line_of(text: &str, err: &toml::de::Error) -> Option<usize> {
    let start = err.span()?.start.min(text.len());
    Some(
        text.as_bytes()[..start]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1,
    )
}

/// `line N: `, to start a message, or nothing.
fn at(text: &str, err: &toml::de::Error) -> String {
    line_of(text, err).map_or(String::new(), |line| format!("line {line}: "))
}

/// ` at line N`, to end a phrase, or nothing.
fn at_line(text: &str, err: &toml::de::Error) -> String {
    line_of(text, err).map_or(String::new(), |line| format!(" at line {line}"))
}

/// The name in `prefix<name>`, in toml's messages. Field names come from the schema or from a key
/// in the config, never from a value.
fn quoted_after<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = message.strip_prefix(prefix)?;
    rest.split('`').next()
}

/// The key that starts the line an error points at, if the line is `key = ...` or `[table]`, and
/// the key is written in the characters a bare or quoted TOML key uses.
fn key_on_line<'a>(text: &'a str, err: &toml::de::Error) -> Option<&'a str> {
    let start = err.span()?.start.min(text.len());
    // Bytes, not str slicing, so a span inside a character cannot panic.
    let line_start = text.as_bytes()[..start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let line = text.get(line_start..)?.lines().next()?.trim();
    let key = match line.strip_prefix('[') {
        Some(header) => header.trim_start_matches('[').split(']').next()?,
        None => line.split('=').next()?,
    }
    .trim();
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b' ');
    (!key.is_empty() && key.bytes().all(plain)).then_some(key)
}
