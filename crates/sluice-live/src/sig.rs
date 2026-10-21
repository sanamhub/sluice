//! The signed message of `SPEC.md` section 6: a label, a `0x00` byte, the `stream_id`, and the
//! record before its signature.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

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
