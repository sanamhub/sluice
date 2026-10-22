//! Rule 8: a key in a ticket is an Ed25519 public key in its one encoding, and not of small order.
//! sluice-live has the same check, and the same cases, for the `sender_key` it is given.

use ed25519_dalek::VerifyingKey;

use super::super::{Body, Format, Ticket, key};
use super::common::*;

/// Every way to spell a point with bytes RFC 8032 section 5.1.3 refuses: a `y` of `p`, `p + 1`
/// and `2^255 - 1` (the largest that fits in 255 bits) with each sign bit, and the sign bit set on
/// the two points whose `x` is 0.
const NON_CANONICAL: [&str; 8] = [
    "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "0100000000000000000000000000000000000000000000000000000000000080",
    "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
];

/// The eight points of small order, each in its one encoding.
const SMALL_ORDER: [&str; 8] = [
    "0100000000000000000000000000000000000000000000000000000000000000",
    "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000080",
    "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
    "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
    "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
    "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
];

/// The public key of RFC 8032 section 7.1 TEST 1.
const VALID: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

fn bytes(hex: &str) -> [u8; 32] {
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

#[test]
fn a_key_in_its_one_encoding_and_of_large_order_is_valid() {
    assert!(key::is_valid(&bytes(VALID)));
    assert!(key::is_valid(&ID));
    assert!(key::is_valid(&KEY));
}

#[test]
fn every_non_canonical_spelling_is_refused_though_ed25519_dalek_decodes_it() {
    for hex in NON_CANONICAL {
        let raw = bytes(hex);
        assert!(VerifyingKey::from_bytes(&raw).is_ok(), "{hex}");
        assert!(!key::is_valid(&raw), "{hex}");
    }
    // A `y` of 2^255 - 1 names the point whose `y` is 18, which is of large order, so only the
    // canonical check refuses it.
    let largest = VerifyingKey::from_bytes(&bytes(NON_CANONICAL[4])).expect("a point");
    assert!(!largest.is_weak());
}

#[test]
fn every_point_of_small_order_is_refused() {
    for hex in SMALL_ORDER {
        let raw = bytes(hex);
        assert_eq!(
            VerifyingKey::from(
                VerifyingKey::from_bytes(&raw)
                    .expect("a point")
                    .to_edwards()
            )
            .to_bytes(),
            raw,
            "{hex} is in its one encoding"
        );
        assert!(!key::is_valid(&raw), "{hex}");
    }
}

#[test]
fn bytes_that_are_no_point_are_refused() {
    assert!(VerifyingKey::from_bytes(&[0x44; 32]).is_err());
    assert!(!key::is_valid(&[0x44; 32]));
}

#[test]
fn a_ticket_that_names_a_bad_key_is_refused_by_new_and_by_decode() {
    let bad = NON_CANONICAL
        .iter()
        .chain(&SMALL_ORDER)
        .map(|hex| bytes(hex))
        .chain([[0x44; 32]]);
    for raw in bad {
        for (endpoint_id, sender_key, field) in [(raw, KEY, "endpoint_id"), (ID, raw, "sender_key")]
        {
            let body = Body::Live {
                stream_id: [0x33; 16],
                sender_key,
            };
            let built = Ticket::new(endpoint_id, vec![], None, None, body);
            let err = built.expect_err("Ticket::new refuses it").to_string();
            assert!(err.contains(field), "{err}");
            let mut ticket_bytes = vec![1, 1];
            ticket_bytes.extend(endpoint_id);
            ticket_bytes.extend([0, 0, 0]);
            ticket_bytes.extend([0x33; 16]);
            ticket_bytes.extend(sender_key);
            let message = refusal_of(&ticket_bytes);
            assert!(message.contains(field), "{message}");
            assert!(message.contains("canonical Ed25519 key"), "{message}");
        }
    }
}

#[test]
fn a_static_root_is_no_key_and_may_be_any_bytes() {
    let body = Body::Static {
        root: [0x44; 32],
        size: 1,
        format: Format::Raw,
    };
    let ticket = Ticket::new(ID, vec![], None, None, body).expect("a ticket");
    assert_eq!(Ticket::decode(&ticket.encode()).expect("decodes"), ticket);
}
