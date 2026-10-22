//! The signed message of `SPEC.md` section 6: a label, a `0x00` byte, the `stream_id`, and the
//! record before its signature.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

#[cfg(test)]
mod tests;

/// One label per kind of record, so a signature for one kind is never good for another.
#[derive(Clone, Copy)]
pub(crate) enum Label {
    Open,
    Entry,
    Close,
}

impl Label {
    fn text(self) -> &'static [u8] {
        match self {
            Self::Open => b"sluice/live/open/1",
            Self::Entry => b"sluice/live/entry/1",
            Self::Close => b"sluice/live/close/1",
        }
    }
}

fn message(label: Label, stream_id: &[u8; 16], body: &[u8]) -> Vec<u8> {
    let label = label.text();
    let mut out = Vec::with_capacity(label.len() + 1 + stream_id.len() + body.len());
    out.extend_from_slice(label);
    out.push(0);
    out.extend_from_slice(stream_id);
    out.extend_from_slice(body);
    out
}

/// The ticket's `sender_key` as a key, when it is one a signature can come from: in its one
/// encoding and not of small order (rule 3). `from_bytes` decodes by ZIP-215, which also takes a
/// `y` of `p` or more and a sign bit on an `x` of 0, where RFC 8032 section 5.1.3 refuses both. So
/// the point is encoded again, and the bytes are canonical when they come back the same. Without
/// that, two receivers could disagree on whether one ticket names a key.
///
/// sluice-core checks the ticket by the same rule. This crate does not depend on it, so the check
/// is written twice, with the same tests.
pub(crate) fn sender_key(bytes: &[u8; 32]) -> Option<VerifyingKey> {
    let key = VerifyingKey::from_bytes(bytes).ok()?;
    let canonical = VerifyingKey::from(key.to_edwards()).as_bytes() == bytes;
    (canonical && !key.is_weak()).then_some(key)
}

pub(crate) fn sign(key: &SigningKey, label: Label, stream_id: &[u8; 16], body: &[u8]) -> [u8; 64] {
    key.sign(&message(label, stream_id, body)).to_bytes()
}

/// Strict verification: it refuses an `s` that is not reduced, a non-canonical `R`, and an `R` or
/// a key of small order, so a signature has one spelling and a weak key verifies nothing.
pub(crate) fn verifies(
    key: &VerifyingKey,
    label: Label,
    stream_id: &[u8; 16],
    body: &[u8],
    sig: &[u8; 64],
) -> bool {
    let message = message(label, stream_id, body);
    key.verify_strict(&message, &Signature::from_bytes(sig))
        .is_ok()
}
