//! The sender refuses what would break a sender rule, and signs exactly the message of section 6.

mod common;

use common::*;
use sluice_live::{Error, Fault, MAX_SEGMENT_BYTES, MIN_SEGMENT_BYTES, Outcome, Sender};

fn sender() -> Sender {
    Sender::new(key(), STREAM, S, 1_800_000_000_000).expect("valid")
}

#[test]
fn the_sender_signs_the_records_the_spec_describes() {
    // Ed25519 signatures are deterministic, so the sender's records equal the ones the tests
    // sign by the text of section 6.
    let mut sender = sender();
    assert_eq!(sender.log_open(), open());
    let frames = chain(&[S, S, 3]);
    for f in &frames {
        assert_eq!(sender.seal(f.entry.segment_root, f.entry.len), Ok(*f));
    }
    assert_eq!(
        sender.close([0xf0; 32], 1_800_000_000_001),
        close_of(&frames)
    );
}

#[test]
fn an_empty_stream_closes_on_the_hash_of_the_open() {
    let close = sender().close([0xf0; 32], 1_800_000_000_001);
    assert_eq!(close, common::close(0, open().hash()));
}

#[test]
fn segment_bytes_out_of_bounds_is_refused() {
    let new = |size| Sender::new(key(), STREAM, size, 0).map(|_| ());
    assert_eq!(new(MIN_SEGMENT_BYTES), Ok(()));
    assert_eq!(new(MAX_SEGMENT_BYTES), Ok(()));
    for size in [
        0,
        MIN_SEGMENT_BYTES - (16 << 10),
        MIN_SEGMENT_BYTES + 1,
        MAX_SEGMENT_BYTES + (16 << 10),
    ] {
        assert_eq!(new(size), Err(Error::Policy(Fault::SegmentBytes)), "{size}");
    }
}

#[test]
fn a_len_out_of_bounds_or_after_a_short_segment_is_refused_and_changes_nothing() {
    let mut sender = sender();
    assert_eq!(sender.seal([1; 32], 0), Err(Error::Policy(Fault::Len)));
    assert_eq!(sender.seal([1; 32], S + 1), Err(Error::Policy(Fault::Len)));
    let first = sender.seal([1; 32], S).expect("full");
    assert_eq!(first.entry.seq, 0);
    let short = sender.seal([2; 32], 1).expect("the last");
    assert_eq!(short.entry.seq, 1);
    assert_eq!(
        sender.seal([3; 32], S),
        Err(Error::Policy(Fault::AfterShort))
    );
    assert_eq!(
        sender.seal([3; 32], 1),
        Err(Error::Policy(Fault::AfterShort))
    );
    let close = sender.close([9; 32], 0);
    assert_eq!(close.total_len, u64::from(S) + 1);
    assert_eq!(close.last_entry_hash, short.entry.hash());
}

#[test]
fn what_the_sender_makes_the_receiver_takes() {
    let mut sender = sender();
    let mut receiver = sluice_live::Receiver::new(STREAM, public(), &sender.log_open().encode())
        .expect("the sender's open");
    for len in [S, S, 77] {
        let f = sender.seal([len as u8; 32], len).expect("a segment");
        assert!(matches!(
            receiver.frame(&f.encode()),
            Outcome::Accepted { .. }
        ));
    }
    let close = sender.close([5; 32], 0);
    let total_len = 2 * u64::from(S) + 77;
    let final_root = [5; 32];
    assert_eq!(
        receiver.close(&close.encode()),
        Outcome::Closed {
            final_root,
            total_len
        }
    );
}

#[test]
fn debug_leaves_the_key_out() {
    let text = format!("{:?}", sender());
    assert!(text.starts_with("Sender"), "{text}");
    assert!(!text.contains("157") && !text.contains("9d"), "{text}");
}
