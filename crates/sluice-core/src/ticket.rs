//! The ticket: the string a user copies to fetch content, as `SPEC.md` section 4 defines it.
//!
//! A ticket is a read capability, so it is treated as a secret. [`Ticket`] has no `Display` and no
//! `Serialize`, its `Debug` hides everything but the kind and the endpoint id, and the errors of
//! [`Ticket::decode`] never quote the text they refused.

mod base32;
mod key;
mod relay;
#[cfg(test)]
mod tests;
mod wire;

use std::fmt;
use std::net::SocketAddr;

use crate::Error;

/// The text every ticket starts with. A decoder ignores its case.
const PREFIX: &str = "sluice1";
/// The longest ticket text, in bytes, that is encoded or decoded.
const MAX_TEXT_BYTES: usize = 4096;
/// The only `version` byte this release reads and writes.
const VERSION: u8 = 1;

/// `Error::Policy` for a ticket that breaks a rule. The reason never holds a part of the ticket.
fn refuse(reason: &str) -> Error {
    Error::Policy(format!("ticket {reason}"))
}

/// The short form of an endpoint id: its first ten base32 characters, in the alphabet a ticket is
/// written in, so a fingerprint and a ticket read alike. `hex` is the 64-character form `sluice id`
/// prints; text that is not that is shortened as it is, because a fingerprint is for a person to
/// read and never for a lookup.
///
/// # Examples
///
/// ```
/// use sluice_core::ticket::fingerprint;
///
/// assert_eq!(fingerprint(&"00".repeat(32)), "0000000000");
/// assert_eq!(fingerprint(&"ff".repeat(32)), "zzzzzzzzzz");
/// ```
pub fn fingerprint(hex: &str) -> String {
    let bytes: Option<Vec<u8>> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    match bytes {
        Some(bytes) if !bytes.is_empty() => base32::encode(&bytes).chars().take(10).collect(),
        _ => hex.chars().take(10).collect(),
    }
}

/// How the bytes of a [`Body::Static`] are laid out.
///
/// # Examples
///
/// ```
/// use sluice_core::ticket::Format;
///
/// assert_ne!(Format::Raw, Format::HashSequence);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Format {
    /// One blob of bytes, wire value 0.
    Raw = 0,
    /// A sequence of hashes of other blobs, wire value 1.
    HashSequence = 1,
}

/// What the ticket names: a finished blob, or a live stream.
///
/// `Debug` hides the contents. A root or a stream id in a log is as bad as the ticket.
///
/// # Examples
///
/// ```
/// use sluice_core::ticket::{Body, Format};
///
/// let body = Body::Static { root: [7; 32], size: 1024, format: Format::Raw };
/// assert_eq!(format!("{body:?}"), "Static(<redacted>)");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub enum Body {
    /// Content with a BLAKE3 root, `kind` 0.
    Static {
        /// The BLAKE3 root of the content.
        root: [u8; 32],
        /// The size of the content in bytes.
        size: u64,
        /// How the bytes are laid out.
        format: Format,
    },
    /// A growing file, `kind` 1.
    Live {
        /// The id of the stream, 16 bytes.
        stream_id: [u8; 16],
        /// The Ed25519 key that signs the stream.
        sender_key: [u8; 32],
    },
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Body::Static { .. } => f.write_str("Static(<redacted>)"),
            Body::Live { .. } => f.write_str("Live(<redacted>)"),
        }
    }
}

/// The complete address of transferable content. Whoever holds one can read the content.
///
/// A `Ticket` is always one that [`Ticket::decode`] would accept, because [`Ticket::new`] applies
/// the same rules, so `decode(&t.encode())` gives back `t`.
///
/// # Examples
///
/// ```
/// use sluice_core::ticket::{Body, Format, Ticket};
///
/// let ticket = Ticket::new(
///     [1; 32],
///     vec!["192.0.2.1:50000".parse()?],
///     None,
///     Some(1_800_000_000_000),
///     Body::Static { root: [2; 32], size: 100 << 20, format: Format::Raw },
/// )?;
/// let text = ticket.encode();
/// assert!(text.starts_with("sluice1"));
/// assert_eq!(Ticket::decode(&text)?, ticket);
/// assert_eq!(Ticket::decode(&text.to_uppercase())?, ticket);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Ticket {
    endpoint_id: [u8; 32],
    addrs: Vec<SocketAddr>,
    relay_url: Option<String>,
    expiry: Option<u64>,
    body: Body,
}

impl Ticket {
    /// A ticket for the provider `endpoint_id`, reachable at `addrs` and through `relay_url`, that
    /// stops working at `expiry` (UTC milliseconds since the Unix epoch) if there is one.
    ///
    /// Fails with [`Error::Policy`] if `relay_url` is an empty string, which would be a second
    /// spelling of no relay, or is not an absolute `https` or `http` URL in its one spelling. That
    /// is rule 7 of `SPEC.md` section 4: at most 2048 bytes, a host, no user name, password, query
    /// or fragment, and equal to its serialization by the WHATWG URL standard. So it is
    /// `https://relay.example/` and not `https://relay.example`. It also fails if the ticket would
    /// be longer than 4096 bytes as text, or if `endpoint_id` or a Live body's `sender_key` is not
    /// an Ed25519 public key in its one encoding, or is one of small order (rule 8).
    ///
    /// The message of a refused relay URL names the form it expected. That is safe here because
    /// the text came from the caller and not from a ticket.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ticket::{Body, Ticket};
    ///
    /// let live = Body::Live { stream_id: [3; 16], sender_key: [3; 32] };
    /// let with = |relay: Option<&str>| {
    ///     Ticket::new([1; 32], vec![], relay.map(String::from), None, live.clone())
    /// };
    /// assert!(with(None).is_ok());
    /// assert!(with(Some("https://relay.example/")).is_ok());
    /// assert!(with(Some("https://relay.example")).is_err());
    /// assert!(with(Some("")).is_err());
    /// ```
    pub fn new(
        endpoint_id: [u8; 32],
        addrs: Vec<SocketAddr>,
        relay_url: Option<String>,
        expiry: Option<u64>,
        body: Body,
    ) -> Result<Self, Error> {
        if relay_url.as_deref() == Some("") {
            return Err(refuse("has an empty relay_url, which absence already says"));
        }
        if let Some(fault) = relay_url.as_deref().and_then(|r| relay::check(r).err()) {
            return Err(refuse(&format!(
                "{}; it must be an absolute https or http URL written as the URL standard \
                 serializes it, such as https://relay.example/",
                fault.describe()
            )));
        }
        key::check(&endpoint_id, &body)?;
        let ticket = Self {
            endpoint_id,
            addrs,
            relay_url,
            expiry,
            body,
        };
        if ticket.encode().len() > MAX_TEXT_BYTES {
            return Err(refuse("would be longer than 4096 bytes"));
        }
        Ok(ticket)
    }

    /// The ticket text: `sluice1`, then the bytes of `SPEC.md` section 4 in lower-case Crockford
    /// base32. One ticket has one text, so this is also the form to compare and to store.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ticket::{Body, Ticket};
    ///
    /// let body = Body::Live { stream_id: [0; 16], sender_key: [1; 32] };
    /// let ticket = Ticket::new([1; 32], vec![], None, None, body)?;
    /// // 85 bytes: version, kind, the key, an empty address list, two absent options and the
    /// // 48 bytes of the body. That is 136 characters of base32.
    /// assert_eq!(ticket.encode().len(), "sluice1".len() + 136);
    /// assert!(ticket.encode().starts_with("sluice1040"));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn encode(&self) -> String {
        format!("{PREFIX}{}", base32::encode(&wire::write(self)))
    }

    /// Reads a ticket from text a person pasted or a peer sent.
    ///
    /// The prefix and the body are read without regard to case. Nothing else is forgiven: a
    /// character outside the base32 alphabet (so no hyphen and no `I`, `L`, `O` or `U`), unused
    /// bits that are not zero, an unknown version, kind or format, bytes after the body, a varint
    /// longer than it needs to be, an empty `relay_url` or one that is not an absolute `https` or
    /// `http` URL in its one spelling, a key that is not an Ed25519 public key in its one encoding
    /// or is of small order, or text over 4096 bytes all fail with [`Error::Policy`]. The message
    /// names the rule and never quotes the text, or any part of it.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, ticket::Ticket};
    ///
    /// let err = Ticket::decode("sluice1-").unwrap_err();
    /// assert!(matches!(err, Error::Policy(_)));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "policy denied: ticket has a character outside the alphabet at byte 0"
    /// );
    /// ```
    pub fn decode(text: &str) -> Result<Self, Error> {
        // Before anything is decoded, so a long string costs one comparison.
        if text.len() > MAX_TEXT_BYTES {
            return Err(refuse("is longer than 4096 bytes"));
        }
        let body = match text.get(..PREFIX.len()) {
            Some(start) if start.eq_ignore_ascii_case(PREFIX) => &text[PREFIX.len()..],
            _ => return Err(refuse("does not start with sluice1")),
        };
        wire::read(&base32::decode(body)?)
    }

    /// Whether the ticket has expired at `now_utc_ms`, in UTC milliseconds since the Unix epoch.
    /// The boundary is the one of a JWT `exp`: a ticket is expired when `now_utc_ms` is equal to
    /// or greater than its expiry, so at `expiry - 1` it is still good and at `expiry` it is not.
    /// One with no expiry never is. The ticket does not read the clock: the caller passes the time
    /// in.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ticket::{Body, Ticket};
    ///
    /// let body = Body::Live { stream_id: [0; 16], sender_key: [1; 32] };
    /// let ticket = Ticket::new([1; 32], vec![], None, Some(1000), body)?;
    /// assert!(!ticket.is_expired(999));
    /// assert!(ticket.is_expired(1000));
    /// assert!(ticket.is_expired(1001));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn is_expired(&self, now_utc_ms: u64) -> bool {
        self.expiry.is_some_and(|expiry| now_utc_ms >= expiry)
    }

    /// The Ed25519 public key of the provider.
    pub fn endpoint_id(&self) -> &[u8; 32] {
        &self.endpoint_id
    }

    /// The direct addresses the provider may be reached at. Can be empty.
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }

    /// The relay to reach the provider through, if the ticket names one. Never empty, and always
    /// an absolute `https` or `http` URL in its serialized form.
    pub fn relay_url(&self) -> Option<&str> {
        self.relay_url.as_deref()
    }

    /// When the ticket expires, in UTC milliseconds since the Unix epoch, if it does.
    pub fn expiry_ms(&self) -> Option<u64> {
        self.expiry
    }

    /// What the ticket names.
    pub fn body(&self) -> &Body {
        &self.body
    }
}

/// Prints the kind and the endpoint id, which is a public key, and hides the rest. A ticket in a
/// log is a read capability that leaked, and `{:?}` is what an `unwrap` or a `tracing` field
/// prints without anyone choosing to.
impl fmt::Debug for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.body {
            Body::Static { .. } => "static",
            Body::Live { .. } => "live",
        };
        let id: String = self
            .endpoint_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        f.debug_struct("Ticket")
            .field("kind", &kind)
            .field("endpoint_id", &id)
            .field("rest", &format_args!("<redacted>"))
            .finish()
    }
}
