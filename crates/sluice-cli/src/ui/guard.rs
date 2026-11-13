//! The checks every request to the page's listener passes, each a function of the request's own
//! values so a test can try any of them. They are the points of ADR 0036 sections 1, 3 and 4: the
//! peer is on loopback, the key matches, the `Host` and the `Origin` are the page's own, the body of
//! a POST is JSON, and it is not larger than 64 KiB.

use std::net::SocketAddr;

/// The most a request body may be. A request is a few paths or a ticket.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// A request that is refused, with the status and the sentence the answer carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reject {
    pub status: u16,
    pub message: &'static str,
}

const fn reject(status: u16, message: &'static str) -> Reject {
    Reject { status, message }
}

/// Whether the peer of a connection is on loopback. A connection from anywhere else is closed
/// before a byte of it is read.
pub fn is_loopback_peer(peer: SocketAddr) -> bool {
    peer.ip().is_loopback()
}

/// The `Host` values the page is served under: the loopback address and `localhost`, on the port the
/// listener has. Any other name, such as one a hostile page rebound to this address, is refused.
fn own_hosts(port: u16) -> [String; 2] {
    [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
}

/// Refuses a request whose `Host` is not one of the page's own.
pub fn check_host(host: Option<&str>, port: u16) -> Result<(), Reject> {
    match host {
        Some(host)
            if own_hosts(port)
                .iter()
                .any(|own| own.eq_ignore_ascii_case(host)) =>
        {
            Ok(())
        }
        _ => Err(reject(
            421,
            "this address is not one the page is served under",
        )),
    }
}

/// Refuses a request whose `Origin`, when it has one, is not the page's own. A request from a page
/// of another site carries that site's origin.
pub fn check_origin(origin: Option<&str>, port: u16) -> Result<(), Reject> {
    let Some(origin) = origin else {
        return Ok(());
    };
    let own = own_hosts(port).map(|host| format!("http://{host}"));
    if own.iter().any(|own| own.eq_ignore_ascii_case(origin)) {
        Ok(())
    } else {
        Err(reject(
            403,
            "a page of another origin may not use this daemon",
        ))
    }
}

/// Compares two byte strings in time that depends only on their length, so a wrong key does not
/// tell by how long the answer takes how much of it was right.
pub fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for (x, y) in a.iter().zip(b) {
        difference |= x ^ y;
    }
    difference == 0
}

/// Refuses a request that does not carry the key as `Authorization: Bearer <key>`.
pub fn check_key(authorization: Option<&str>, key: &str) -> Result<(), Reject> {
    let given = authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if same_bytes(given.as_bytes(), key.as_bytes()) {
        Ok(())
    } else {
        Err(reject(401, "the request does not carry the key"))
    }
}

/// Refuses a POST whose content type is not `application/json`, with or without a charset. A form
/// or a plain-text body is what another site's page can send without a preflight.
pub fn check_json(content_type: Option<&str>) -> Result<(), Reject> {
    let media = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    match media {
        Some(media) if media.eq_ignore_ascii_case("application/json") => Ok(()),
        _ => Err(reject(
            415,
            "a request body is JSON, sent as application/json",
        )),
    }
}

/// Refuses a body that says it is larger than the limit, before any of it is read. A request with
/// no length is read through a limit of the same size.
pub fn check_length(content_length: Option<&str>) -> Result<(), Reject> {
    match content_length.map(|value| value.trim().parse::<usize>()) {
        Some(Ok(length)) if length > MAX_BODY_BYTES => {
            Err(reject(413, "the request body is larger than 64 KiB"))
        }
        Some(Err(_)) => Err(reject(
            400,
            "the length of the request body is not a number",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
