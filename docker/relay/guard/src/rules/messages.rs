//! The refusal for text that is TOML but not iroh-relay 1.4.0's config. It names the line, and the
//! key on it when the line has one, but never what toml quoted from the value.

use super::README;
use crate::Refusal;

pub(super) fn schema_error(text: &str, err: &toml::de::Error) -> Refusal {
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
pub(super) fn at_line(text: &str, err: &toml::de::Error) -> String {
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
