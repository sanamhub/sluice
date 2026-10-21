#![no_main]

use std::sync::OnceLock;

use ed25519_dalek::SigningKey;
use libfuzzer_sys::fuzz_target;
use sluice_live::{Frame, LogOpen, Outcome, Receiver, Sender, Status};

/// The stream and key of the `live/` vectors, so their frames and closes seed this target as they
/// are: the stream `5a` repeated, the published secret key of RFC 8032 section 7.1 TEST 1, and
/// segments of 1 MiB.
const STREAM: [u8; 16] = [0x5a; 16];
const SEGMENT_BYTES: u32 = 1 << 20;
const STARTED: u64 = 1_800_000_000_000;

fn open() -> &'static (LogOpen, [u8; 32]) {
    static OPEN: OnceLock<(LogOpen, [u8; 32])> = OnceLock::new();
    OPEN.get_or_init(|| {
        let key = SigningKey::from_bytes(&[
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xcc, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ]);
        let public = key.verifying_key().to_bytes();
        let sender = Sender::new(key, STREAM, SEGMENT_BYTES, STARTED).expect("valid");
        (sender.log_open(), public)
    })
}

/// The records of an input: each is a tag byte, whose low bit says frame (0) or close (1), a
/// length byte, and that many bytes, or what is left.
fn records(mut data: &[u8]) -> impl Iterator<Item = (bool, &[u8])> {
    std::iter::from_fn(move || {
        let (&tag, rest) = data.split_first()?;
        let (&len, rest) = rest.split_first().unwrap_or((&0, &[]));
        let (record, rest) = rest.split_at(usize::from(len).min(rest.len()));
        data = rest;
        Some((tag & 1 == 1, record))
    })
}

/// What the chain must look like, kept apart from the receiver: the head, the next `seq`, whether
/// the last entry was short, and the sum of the lengths.
struct Chain {
    head: [u8; 32],
    next: u64,
    short: bool,
    total: u64,
}

// Whatever the records are, the receiver never panics, a refusal or a repeat leaves it exactly as
// it was, an abort answers every later record the same way, and an accepted entry is always the
// next link: its `seq`, its `prev_entry_hash` and its `len` follow from the entries before it.
fuzz_target!(|data: &[u8]| {
    let (open, public) = open();
    let mut receiver = Receiver::new(STREAM, *public, &open.encode()).expect("a valid open");
    let mut chain = Chain {
        head: open.hash(),
        next: 0,
        short: false,
        total: 0,
    };
    let mut aborted = None;
    for (is_close, bytes) in records(data) {
        let before = receiver.clone();
        let was_closed = matches!(before.status(), Status::Closed { .. });
        let outcome = if is_close {
            receiver.close(bytes)
        } else {
            receiver.frame(bytes)
        };
        if let Some(abort) = aborted {
            assert_eq!(outcome, abort, "an abort that did not stick");
        }
        match outcome {
            Outcome::Refused(_) | Outcome::Repeat { .. } => assert_eq!(receiver, before),
            Outcome::Closed { .. } if was_closed => assert_eq!(receiver, before),
            Outcome::Accepted { seq, len, .. } => {
                let frame = Frame::decode(bytes).expect("an accepted frame decodes");
                assert_eq!(seq, chain.next, "not the next seq");
                assert_eq!(
                    frame.entry.prev_entry_hash, chain.head,
                    "not chained to the head"
                );
                assert!(
                    (1..=SEGMENT_BYTES).contains(&len) && !chain.short,
                    "a len out of rule"
                );
                chain.head = frame.entry.hash();
                chain.next += 1;
                chain.short = len < SEGMENT_BYTES;
                chain.total += u64::from(len);
                assert_eq!(
                    receiver.status(),
                    Status::Open {
                        next_seq: chain.next
                    }
                );
            }
            Outcome::Aborted { seq, .. } => {
                assert!(seq <= chain.next, "an abort past the chain");
                aborted = Some(outcome);
            }
            Outcome::Closed { total_len, .. } => {
                assert_eq!(total_len, chain.total, "a wrong total")
            }
        }
    }
});
