//! The ticket's bytes, field by field, as the table in `SPEC.md` section 4 gives them.
//!
//! Each field goes through `postcard` on its own, and the fields are laid end to end. That is the
//! encoding of one flat struct, and it keeps the body free of a tag of its own: `kind` selects it.
//! It also lets the reader check an address count before it allocates anything for it.

use std::net::SocketAddr;

use serde::{Serialize, de::DeserializeOwned};

use super::{Body, Format, Ticket, VERSION, refuse};
use crate::Error;

/// The shortest address: a tag byte, 4 address bytes and a one-byte port.
const SHORTEST_ADDR: usize = 6;

/// The bytes of a ticket, always in their shortest form.
pub(super) fn write(ticket: &Ticket) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    put(&mut out, &VERSION);
    put(&mut out, &kind_byte(&ticket.body));
    put(&mut out, &ticket.endpoint_id);
    put(&mut out, &(ticket.addrs.len() as u64));
    for addr in &ticket.addrs {
        put(&mut out, addr);
    }
    put(&mut out, &ticket.relay_url);
    put(&mut out, &ticket.expiry);
    match &ticket.body {
        Body::Static { root, size, format } => {
            put(&mut out, root);
            put(&mut out, size);
            put(&mut out, &(*format as u8));
        }
        Body::Live {
            stream_id,
            sender_key,
        } => {
            put(&mut out, stream_id);
            put(&mut out, sender_key);
        }
    }
    out
}

fn put<T: Serialize>(out: &mut Vec<u8>, value: &T) {
    // Postcard fails to a `Vec` only for a type whose `Serialize` returns an error, or for a
    // sequence of unknown length. These are integers, byte arrays, strings and addresses, all of
    // known size, so a failure here is a bug. Skipping the field would write a corrupt ticket
    // with no sign of it, so this stops instead.
    let bytes = postcard::to_allocvec(value).expect("postcard encodes a ticket field into a Vec");
    out.extend_from_slice(&bytes);
}

fn kind_byte(body: &Body) -> u8 {
    match body {
        Body::Static { .. } => 0,
        Body::Live { .. } => 1,
    }
}

/// A ticket from its bytes, or the first rule it breaks. Whatever it returns would write back to
/// exactly these bytes, so a byte string has one ticket and a ticket has one byte string.
pub(super) fn read(bytes: &[u8]) -> Result<Ticket, Error> {
    // The layout can change with the version, so nothing else is read before this byte is known.
    let version = *bytes.first().ok_or_else(|| refuse("is empty"))?;
    if version != VERSION {
        return Err(refuse(&format!("version {version} is not supported")));
    }
    let mut reader = Reader { rest: bytes };
    let _version: u8 = reader.take()?;
    let kind: u8 = reader.take()?;
    if kind > 1 {
        return Err(refuse("kind is not 0 or 1"));
    }
    let endpoint_id = reader.take()?;
    let addrs = reader.addrs()?;
    let relay_url: Option<String> = reader.take()?;
    let expiry = reader.take()?;
    let body = if kind == 0 {
        reader.static_body()?
    } else {
        reader.live_body()?
    };
    if !reader.rest.is_empty() {
        return Err(refuse("has bytes after the body"));
    }
    if relay_url.as_deref() == Some("") {
        return Err(refuse("has an empty relay_url, which absence already says"));
    }
    let ticket = Ticket {
        endpoint_id,
        addrs,
        relay_url,
        expiry,
        body,
    };
    if write(&ticket) != bytes {
        // Postcard accepts a varint with a zero byte too many. Two spellings of one ticket would
        // be two capabilities to anyone who keeps a list of the ones it has refused.
        return Err(refuse("is not in its shortest byte form"));
    }
    Ok(ticket)
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn take<T: DeserializeOwned>(&mut self) -> Result<T, Error> {
        let (value, rest) = postcard::take_from_bytes(self.rest)
            .map_err(|e| refuse(&format!("is cut short or malformed: {e}")))?;
        self.rest = rest;
        Ok(value)
    }

    /// The address list. The count comes from a stranger, so it is checked against the bytes
    /// that are left before it sizes a vector.
    fn addrs(&mut self) -> Result<Vec<SocketAddr>, Error> {
        let count: u64 = self.take()?;
        if count > (self.rest.len() / SHORTEST_ADDR) as u64 {
            return Err(refuse("lists more addresses than it has bytes for"));
        }
        (0..count).map(|_| self.take()).collect()
    }

    fn static_body(&mut self) -> Result<Body, Error> {
        let root = self.take()?;
        let size = self.take()?;
        let format = match self.take::<u8>()? {
            0 => Format::Raw,
            1 => Format::HashSequence,
            _ => return Err(refuse("format is not 0 or 1")),
        };
        Ok(Body::Static { root, size, format })
    }

    fn live_body(&mut self) -> Result<Body, Error> {
        let stream_id = self.take()?;
        let sender_key = self.take()?;
        Ok(Body::Live {
            stream_id,
            sender_key,
        })
    }
}
