//! The refusal for text that is TOML but not iroh-relay 1.4.0's config. It names the line, and the
//! key on it when the key looks like a setting's name, but never what toml quoted from a value.
//!
//! A key the guard does not know is text from the config, and may be a token pasted in the wrong
//! place, or hold a newline or a terminal escape code. Its name is printed only when it is 1 to 32
//! lower-case ASCII letters or `_`, as every iroh-relay 1.4.0 setting is. A hex or base64url token
//! has digits or capitals, or is longer, so it is not printed. Any other is "a key", with the
//! line.

use crate::Refusal;

pub(super) fn schema_error(text: &str, err: &toml::de::Error, readme: &str) -> Refusal {
    let at = at(text, err);
    let message = err.message();
    if message.starts_with("unknown field `") {
        let key = match quoted_after(message, "unknown field `").and_then(printable) {
            Some(name) => format!("unknown key `{name}`"),
            None => "a key".to_string(),
        };
        return Refusal::new(format!(
            "{at}{key}: iroh-relay 1.4.0 has no such setting, and would ignore it; check the \
             spelling against {readme}"
        ));
    }
    // A missing field's name comes from the schema, never from the config.
    if let Some(name) = quoted_after(message, "missing field `").and_then(printable) {
        return Refusal::new(format!(
            "{at}this table needs the key `{name}`: see {readme}"
        ));
    }
    let key = key_on_line(text, err);
    if message.starts_with("unknown variant `") {
        let key = key.map_or("a key".to_string(), |key| format!("`{key}`"));
        return Refusal::new(format!(
            "{at}{key} is not a setting or value iroh-relay 1.4.0 knows here; check the \
             spelling against {readme} (the value is not printed, since it may be a token)"
        ));
    }
    let value = key.map_or("a value".to_string(), |key| format!("the value of `{key}`"));
    Refusal::new(format!(
        "{at}{value} does not have the type iroh-relay 1.4.0 reads; check it against {readme} \
         (the value is not printed, since it may be a token)"
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

/// The name in `prefix<name>`, in toml's messages, which follow it with a backtick and a comma or
/// the end. Field names come from the schema or from a key in the config, never from a value. A
/// name with a backtick in it gives `None`, not the part before the backtick.
fn quoted_after<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = message.strip_prefix(prefix)?;
    let name = rest.split('`').next()?;
    let after = rest.get(name.len()..)?;
    (after == "`" || after.starts_with("`,")).then_some(name)
}

/// `name` if it is 1 to 32 lower-case ASCII letters or `_`, the way every iroh-relay 1.4.0 setting
/// is written, and so cannot be a hex or base64url token, a newline or an escape code.
fn printable(name: &str) -> Option<&str> {
    let plain = |b: u8| b.is_ascii_lowercase() || b == b'_';
    (1..=32)
        .contains(&name.len())
        .then_some(name)
        .filter(|name| name.bytes().all(plain))
}

/// The key that starts the line an error points at, if the line is `key = ...` or `[table]`, and
/// each part of the key, between dots, is [`printable`].
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
    let parts_printable = key.split('.').all(|part| printable(part.trim()).is_some());
    parts_printable.then_some(key)
}
