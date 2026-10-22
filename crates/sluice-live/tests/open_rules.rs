//! Rules 1 to 4: the opening record, checked against the ticket before a stream starts.

mod common;

use common::*;
use sluice_live::{Error, Fault, LogOpen, MAX_SEGMENT_BYTES, MIN_SEGMENT_BYTES, Receiver};

fn start(sender_key: [u8; 32], open: &LogOpen) -> Result<(), Error> {
    Receiver::new(STREAM, sender_key, &open.encode()).map(|_| ())
}

const fn refused(fault: Fault) -> Result<(), Error> {
    Err(Error::Verify(fault))
}

/// The group order of Ed25519, little-endian.
const L: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// `s + L`, which is the same scalar, so the signature holds by the group equation and is not in
/// its one spelling.
fn plus_l(sig: [u8; 64]) -> [u8; 64] {
    let mut out = sig;
    let mut carry = 0u16;
    for (at, l) in L.iter().enumerate() {
        let sum = u16::from(sig[32 + at]) + u16::from(*l) + carry;
        out[32 + at] = sum as u8;
        carry = sum >> 8;
    }
    assert_eq!(carry, 0, "s + L fits 32 bytes, since s is below L");
    out
}

#[test]
fn a_valid_open_starts_the_stream() {
    let receiver = Receiver::new(STREAM, public(), &open().encode()).expect("valid");
    assert_eq!(receiver.segment_bytes(), S);
}

#[test]
fn rule_1_an_open_of_another_length_is_refused() {
    let bytes = open().encode();
    for len in [0, 123] {
        assert!(matches!(
            Receiver::new(STREAM, public(), &bytes[..len]),
            Err(Error::Verify(Fault::Length))
        ));
    }
    let mut long = bytes.to_vec();
    long.push(0);
    assert!(matches!(
        Receiver::new(STREAM, public(), &long),
        Err(Error::Verify(Fault::Length))
    ));
}

#[test]
fn rule_2_an_open_for_another_stream_or_sender_is_refused() {
    let other = Receiver::new([0x5b; 16], public(), &open().encode()).map(|_| ());
    assert_eq!(other, refused(Fault::Ticket));
    // The public key of RFC 8032 TEST 2.
    let test_2: [u8; 32] = [
        0x3d, 0x40, 0x17, 0xc3, 0xe8, 0x43, 0x89, 0x5a, 0x92, 0xb7, 0x0a, 0xa7, 0x4d, 0x1b, 0x7e,
        0xbc, 0x9c, 0x98, 0x2c, 0xcf, 0x2e, 0xc4, 0x96, 0x8c, 0xc0, 0xcd, 0x55, 0xf1, 0x2a, 0xf4,
        0x66, 0x0c,
    ];
    assert_eq!(start(test_2, &open()), refused(Fault::Ticket));
}

#[test]
fn rule_3_an_open_whose_signature_does_not_verify_is_refused() {
    let mut damaged = open();
    damaged.sig[0] ^= 1;
    assert_eq!(start(public(), &damaged), refused(Fault::Signature));
    let mut changed = open();
    changed.started += 1;
    assert_eq!(start(public(), &changed), refused(Fault::Signature));
    // Signed as an entry, and signed for another stream.
    let mut entry_label = open();
    entry_label.sig = sign(ENTRY, &STREAM, &open().encode()[..60]);
    assert_eq!(start(public(), &entry_label), refused(Fault::Signature));
    let mut other_stream = open();
    other_stream.sig = sign(OPEN, &[0x5b; 16], &open().encode()[..60]);
    assert_eq!(start(public(), &other_stream), refused(Fault::Signature));
}

#[test]
fn rule_3_a_signature_out_of_its_one_spelling_is_refused() {
    let mut malleable = open();
    malleable.sig = plus_l(open().sig);
    assert_ne!(malleable.sig, open().sig);
    assert_eq!(start(public(), &malleable), refused(Fault::Signature));
}

#[test]
fn rule_3_a_key_of_small_order_verifies_nothing() {
    // The neutral point. With R the neutral point and s = 0, [s]B = R + [k]A holds for every
    // message, so only the strict check refuses it.
    let mut neutral = [0u8; 32];
    neutral[0] = 1;
    let mut sig = [0u8; 64];
    sig[0] = 1;
    let weak = LogOpen {
        sender_key: neutral,
        sig,
        ..open()
    };
    assert_eq!(start(neutral, &weak), refused(Fault::Signature));
    // Bytes that are no point at all: y = 2 has no x on the curve.
    let mut nothing = [0u8; 32];
    nothing[0] = 2;
    let none = LogOpen {
        sender_key: nothing,
        ..open()
    };
    assert_eq!(start(nothing, &none), refused(Fault::Signature));
}

#[test]
fn rule_3_a_ticket_key_out_of_its_one_encoding_is_refused() {
    // y = 2^255 - 1, which ed25519-dalek reads as the point whose y is 18. Nobody knows its
    // secret, so the unit tests of `sig` show that the encoding check is what refuses it.
    let mut spelled = [0xff; 32];
    spelled[31] = 0x7f;
    let open = LogOpen {
        sender_key: spelled,
        ..open()
    };
    assert_eq!(start(spelled, &open), refused(Fault::Signature));
}

#[test]
fn rule_4_segment_bytes_is_a_multiple_of_16_kib_from_1_to_64_mib() {
    let block = 16 << 10;
    for good in [
        MIN_SEGMENT_BYTES,
        MIN_SEGMENT_BYTES + block,
        8 << 20,
        MAX_SEGMENT_BYTES,
    ] {
        assert_eq!(start(public(), &open_with(good)), Ok(()), "{good}");
    }
    let bad = [
        0,
        block,
        MIN_SEGMENT_BYTES - block,
        MIN_SEGMENT_BYTES + 1,
        MAX_SEGMENT_BYTES - 1,
        MAX_SEGMENT_BYTES + block,
        u32::MAX,
    ];
    for size in bad {
        assert_eq!(
            start(public(), &open_with(size)),
            refused(Fault::SegmentBytes),
            "{size}"
        );
    }
}

#[test]
fn rule_4_is_checked_after_the_signature() {
    let mut unsigned = open_with(0);
    unsigned.sig[5] ^= 1;
    assert_eq!(start(public(), &unsigned), refused(Fault::Signature));
}
