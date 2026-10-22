//! The parts of `access` that iroh-relay reads and the guard checks before it does: the ids of an
//! allowlist or a denylist, the URL of an HTTP check, and the shared tokens.

use crate::Refusal;
use crate::schema::AccessConfig;

pub(super) fn check(access: &AccessConfig, access_token: Option<&str>) -> Result<(), Refusal> {
    match access {
        AccessConfig::Allowlist(ids) => check_ids("allowlist", ids),
        AccessConfig::Denylist(ids) => check_ids("denylist", ids),
        AccessConfig::Http(http) => check_url(&http.url),
        AccessConfig::SharedToken(tokens) => check_tokens(tokens, access_token),
        AccessConfig::Everyone => Ok(()),
    }
}

/// An id is what `sluice id` prints: an ed25519 public key in 64 lower-case hex digits.
/// iroh-base 1.4.0 decodes a 64-character id with `HEXLOWER` and then as a curve point
/// (`src/key.rs:122-127` and `475-482`), and iroh-relay refuses to start on any id it cannot read.
/// It also takes a 52-character base32 spelling, which the README does not offer, so the guard
/// refuses that too. An empty list lets nobody in, and is what the image ships.
fn check_ids(list: &str, ids: &[String]) -> Result<(), Refusal> {
    for (n, id) in ids.iter().enumerate() {
        // By its place, not its text: a token pasted into the wrong list stays unprinted.
        let entry = format!("entry {} of `access.{list}`", n + 1);
        let paste = "paste the id that `sluice id` prints";
        let Some(bytes) = lower_hex(id) else {
            let why = match id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                true => "has upper-case hex digits, which iroh-relay 1.4.0 does not read",
                false => "is not 64 lower-case hex digits",
            };
            return Err(Refusal::new(format!("{entry} {why}: {paste}")));
        };
        if ed25519_dalek::VerifyingKey::from_bytes(&bytes).is_err() {
            return Err(Refusal::new(format!(
                "{entry} is 64 hex digits but not an ed25519 public key, which iroh-relay 1.4.0 \
                 refuses: {paste}"
            )));
        }
    }
    Ok(())
}

/// The 32 bytes of 64 lower-case hex digits, or `None` for anything else.
fn lower_hex(id: &str) -> Option<[u8; 32]> {
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    if id.len() != 64 {
        return None;
    }
    let (pairs, _) = id.as_bytes().as_chunks::<2>();
    let mut bytes = [0u8; 32];
    for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = digit(*high)? << 4 | digit(*low)?;
    }
    Some(bytes)
}

/// iroh-relay logs the URL of a failed HTTP check at `warn` (main.rs:331), since reqwest's error
/// holds it whole, so a user name, password, query or fragment in it ends up in the container log.
/// A secret belongs in `bearer_token`, which goes in a header. The URL is never printed here
/// either, nor any part of it.
fn check_url(url: &str) -> Result<(), Refusal> {
    let why = match url::Url::parse(url) {
        Err(_) => "is not an absolute URL",
        Ok(url) if !matches!(url.scheme(), "http" | "https") => "is not an http or https URL",
        Ok(url) if !url.username().is_empty() || url.password().is_some() => {
            "holds a user name or password"
        }
        Ok(url) if url.query().is_some() => "holds a query",
        Ok(url) if url.fragment().is_some() => "holds a fragment",
        Ok(_) => return Ok(()),
    };
    Err(Refusal::new(format!(
        "`access.http.url` {why}, and iroh-relay logs that URL when a check fails: give an http or \
         https URL with no user name, password, query or fragment, and put a secret in \
         `bearer_token`, which is sent as a header (the URL is not printed)"
    )))
}

/// iroh-relay refuses to start on an empty list or an empty token (main.rs:242-244). A set
/// `IROH_RELAY_ACCESS_TOKEN` replaces the whole list first (main.rs:238-241), so the list is then
/// not read at all; an empty variable is refused before the file is.
fn check_tokens(tokens: &[String], access_token: Option<&str>) -> Result<(), Refusal> {
    if access_token.is_some() {
        return Ok(());
    }
    if tokens.is_empty() {
        return Err(Refusal::new(
            "`access.shared_token` is empty, and iroh-relay 1.4.0 refuses to start without a \
             token: add one, or set IROH_RELAY_ACCESS_TOKEN",
        ));
    }
    match tokens.iter().position(String::is_empty) {
        Some(n) => Err(Refusal::new(format!(
            "entry {} of `access.shared_token` is empty, and iroh-relay 1.4.0 refuses to start \
             with one: remove it, or give it the token clients present",
            n + 1
        ))),
        None => Ok(()),
    }
}
