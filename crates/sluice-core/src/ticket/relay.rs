//! Rule 7 of `SPEC.md` section 4: the one spelling a `relay_url` may have.
//!
//! The URL is parsed with the `url` crate, which implements the WHATWG URL standard and is what
//! iroh's `RelayUrl` parses with. A ticket that passes here is one iroh accepts later, and the
//! check that the text equals its own serialization is what leaves a URL a single spelling.

use url::{ParseError, Url};

/// The longest `relay_url`, in bytes.
pub(super) const MAX_BYTES: usize = 2048;

/// Why a `relay_url` was refused. Each variant is one clause of rule 7, and none holds any of the
/// text it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    TooLong,
    NotAbsolute,
    Scheme,
    NoHost,
    Extras,
    NotSerialized,
}

impl Fault {
    /// The clause that failed, in words for an error message.
    pub(super) fn describe(self) -> &'static str {
        match self {
            Fault::TooLong => "relay_url is longer than 2048 bytes",
            Fault::NotAbsolute => "relay_url is not a valid absolute URL",
            Fault::Scheme => "relay_url does not use the scheme https or http",
            Fault::NoHost => "relay_url has no host",
            Fault::Extras => "relay_url has a user name, a password, a query or a fragment",
            Fault::NotSerialized => "relay_url is not in its serialized form",
        }
    }
}

/// `Ok` if `relay` is a URL rule 7 allows, or the first clause it breaks.
pub(super) fn check(relay: &str) -> Result<(), Fault> {
    if relay.len() > MAX_BYTES {
        return Err(Fault::TooLong);
    }
    let url = Url::parse(relay).map_err(|error| match error {
        // For `https` and `http` the parser itself refuses a missing host, so this is where the
        // "with a host" clause of the rule is enforced.
        ParseError::EmptyHost => Fault::NoHost,
        _ => Fault::NotAbsolute,
    })?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(Fault::Scheme);
    }
    let has_extras = !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some();
    if has_extras {
        return Err(Fault::Extras);
    }
    // After the clauses above, so a URL that is wrong in a way a person can name gets that name.
    // The parser lower-cases the scheme and host, adds the `/` of an empty path, drops a default
    // port and punycodes a host, so equal text means the URL was already written that way.
    if url.as_str() != relay {
        return Err(Fault::NotSerialized);
    }
    Ok(())
}
