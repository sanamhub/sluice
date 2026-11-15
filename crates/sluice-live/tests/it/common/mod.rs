//! A key, a stream and signing written from `SPEC.md` section 6 and not from the crate, so the
//! tests can make records that the sender refuses to make, and check the signed message too.

#![allow(dead_code)]

use ed25519_dalek::{Signer, SigningKey};
use sluice_live::{Frame, LogClose, LogEntry, LogOpen, Receiver};

pub const STREAM: [u8; 16] = [0x5a; 16];
pub const S: u32 = 1 << 20;

/// The secret key of RFC 8032 section 7.1, TEST 1, which is published.
pub fn key() -> SigningKey {
    SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

pub fn public() -> [u8; 32] {
    key().verifying_key().to_bytes()
}

pub const OPEN: &[u8] = b"sluice/live/open/1";
pub const ENTRY: &[u8] = b"sluice/live/entry/1";
pub const CLOSE: &[u8] = b"sluice/live/close/1";

/// The signature of `body` as a record of the kind `label`, in the stream `stream`.
pub fn sign(label: &[u8], stream: &[u8; 16], body: &[u8]) -> [u8; 64] {
    let mut message = label.to_vec();
    message.push(0);
    message.extend_from_slice(stream);
    message.extend_from_slice(body);
    key().sign(&message).to_bytes()
}

pub fn open_with(segment_bytes: u32) -> LogOpen {
    let mut open = LogOpen {
        stream_id: STREAM,
        sender_key: public(),
        segment_bytes,
        started: 1_800_000_000_000,
        sig: [0; 64],
    };
    open.sig = sign(OPEN, &STREAM, &open.encode()[..60]);
    open
}

pub fn open() -> LogOpen {
    open_with(S)
}

pub fn frame(entry: LogEntry) -> Frame {
    Frame {
        entry,
        sig: sign(ENTRY, &STREAM, &entry.encode()),
    }
}

/// The entry `seq` of a chain of full segments, whose roots are `seq` repeated.
pub fn entry(seq: u64, prev: [u8; 32], len: u32) -> LogEntry {
    LogEntry {
        seq,
        segment_root: [seq as u8; 32],
        len,
        prev_entry_hash: prev,
    }
}

/// `count` valid frames of `lens`, chained from `open()`.
pub fn chain(lens: &[u32]) -> Vec<Frame> {
    let mut prev = open().hash();
    lens.iter()
        .enumerate()
        .map(|(seq, &len)| {
            let f = frame(entry(seq as u64, prev, len));
            prev = f.entry.hash();
            f
        })
        .collect()
}

pub fn close(total_len: u64, last_entry_hash: [u8; 32]) -> LogClose {
    let mut close = LogClose {
        final_root: [0xf0; 32],
        total_len,
        closed: 1_800_000_000_001,
        last_entry_hash,
        sig: [0; 64],
    };
    close.sig = sign(CLOSE, &STREAM, &close.encode()[..80]);
    close
}

/// The close of `frames`, or of an empty stream.
pub fn close_of(frames: &[Frame]) -> LogClose {
    let total = frames.iter().map(|f| u64::from(f.entry.len)).sum();
    let last = frames.last().map_or(open().hash(), |f| f.entry.hash());
    close(total, last)
}

pub fn receiver() -> Receiver {
    Receiver::new(STREAM, public(), &open().encode()).expect("a valid open")
}

/// A receiver that has accepted `frames`.
pub fn fed(frames: &[Frame]) -> Receiver {
    let mut r = receiver();
    for f in frames {
        assert!(
            matches!(r.frame(&f.encode()), sluice_live::Outcome::Accepted { .. }),
            "a frame of the chain"
        );
    }
    r
}
