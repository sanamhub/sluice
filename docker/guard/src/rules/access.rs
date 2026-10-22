//! The parts of `access` that iroh-relay reads and the guard checks before it does: the ids of an
//! allowlist, and the URL of an HTTP check.

use crate::Refusal;
use crate::schema::AccessConfig;

pub(super) fn check(access: &AccessConfig) -> Result<(), Refusal> {
    match access {
        AccessConfig::Allowlist(ids) => check_ids(ids),
        AccessConfig::Http(http) => check_url(&http.url),
        AccessConfig::Everyone | AccessConfig::Denylist(_) | AccessConfig::SharedToken(_) => Ok(()),
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
