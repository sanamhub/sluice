//! Rule 8 of `SPEC.md` section 4: a key in a ticket is an Ed25519 public key in its one encoding,
//! and not of small order.

use ed25519_dalek::VerifyingKey;

use super::{Body, refuse};
use crate::Error;

/// Rule 8 for each key of a ticket: the endpoint id, then a Live ticket's sender key.
pub(super) fn check(endpoint_id: &[u8; 32], body: &Body) -> Result<(), Error> {
    if !is_valid(endpoint_id) {
        return Err(refuse(
            "has an endpoint_id that is not a canonical Ed25519 key of large order",
        ));
    }
    if let Body::Live { sender_key, .. } = body
        && !is_valid(sender_key)
    {
        return Err(refuse(
            "has a sender_key that is not a canonical Ed25519 key of large order",
        ));
    }
    Ok(())
}

/// Whether `bytes` is a key a ticket may name. `from_bytes` decodes by ZIP-215, which also takes a
/// `y` of `p` or more and a sign bit on an `x` of 0, where RFC 8032 section 5.1.3 refuses both. So
/// the point is encoded again, and the bytes are canonical when they come back the same. Without
/// that, two decoders could disagree on whether one ticket is valid.
///
/// A key of small order is refused too: no signature from it verifies strictly, so a ticket that
/// names one could only lead to a provider or a stream nothing can trust.
pub(super) fn is_valid(bytes: &[u8; 32]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(bytes) else {
        return false;
    };
    VerifyingKey::from(key.to_edwards()).as_bytes() == bytes && !key.is_weak()
}
