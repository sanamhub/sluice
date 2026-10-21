//! Rules 1, 3 and 5 to 11 for frames: a refused frame changes nothing, and a frame the sender
//! signed that breaks the chain aborts the stream at its `seq`.

mod common;

use common::*;
use sluice_live::{Fault, Frame, Outcome, Status};

fn aborted(seq: u64, reason: Fault) -> Outcome {
    Outcome::Aborted { seq, reason }
}

/// Gives `bytes` to a receiver that took `frames`, checks a refusal leaves it as it was, and
/// returns the outcome.
fn after(frames: &[Frame], bytes: &[u8]) -> Outcome {
    let mut receiver = fed(frames);
    let before = receiver.clone();
    let outcome = receiver.frame(bytes);
    if matches!(outcome, Outcome::Refused(_) | Outcome::Repeat { .. }) {
        assert_eq!(receiver, before, "a refusal changed the receiver");
    }
    outcome
}

#[test]
fn a_chain_is_accepted_in_order() {
    let frames = chain(&[S, S, 7]);
    let mut receiver = receiver();
    for f in &frames {
        let want = Outcome::Accepted {
            seq: f.entry.seq,
            segment_root: f.entry.segment_root,
            len: f.entry.len,
        };
        assert_eq!(receiver.frame(&f.encode()), want);
    }
    assert_eq!(receiver.status(), Status::Open { next_seq: 3 });
}

#[test]
fn rule_1_a_frame_of_another_length_is_refused() {
    let bytes = chain(&[S])[0].encode();
    for len in [0, 76, 139] {
        assert_eq!(after(&[], &bytes[..len]), Outcome::Refused(Fault::Length));
    }
    let mut long = bytes.to_vec();
    long.push(0);
    assert_eq!(after(&[], &long), Outcome::Refused(Fault::Length));
}

#[test]
fn rule_3_a_frame_that_does_not_verify_is_refused_and_the_right_one_still_counts() {
    let frames = chain(&[S, S]);
    // Not in `seq`: a frame with another `seq` is ahead, or below the next one.
    for at in [8, 39, 40, 43, 44, 75, 76, 139] {
        let mut bytes = frames[1].encode();
        bytes[at] ^= 0x80;
        assert_eq!(
            after(&frames[..1], &bytes),
            Outcome::Refused(Fault::Signature),
            "{at}"
        );
    }
    let entry = frames[0].entry;
    let labelled = |label: &[u8], stream: &[u8; 16]| Frame {
        entry,
        sig: sign(label, stream, &entry.encode()),
    };
    for wrong in [
        labelled(OPEN, &STREAM),
        labelled(CLOSE, &STREAM),
        labelled(ENTRY, &[1; 16]),
    ] {
        assert_eq!(
            after(&[], &wrong.encode()),
            Outcome::Refused(Fault::Signature)
        );
    }
    let mut receiver = receiver();
    let mut damaged = frames[0].encode();
    damaged[100] ^= 1;
    receiver.frame(&damaged);
    assert!(matches!(
        receiver.frame(&frames[0].encode()),
        Outcome::Accepted { seq: 0, .. }
    ));
}

#[test]
fn rule_5_a_frame_ahead_of_the_next_is_refused_unchecked() {
    let frames = chain(&[S, S, S]);
    assert_eq!(
        after(&[], &frames[1].encode()),
        Outcome::Refused(Fault::Ahead)
    );
    let mut unsigned = frames[2];
    unsigned.sig = [0; 64];
    assert_eq!(
        after(&frames[..1], &unsigned.encode()),
        Outcome::Refused(Fault::Ahead)
    );
    assert!(matches!(
        after(&frames[..1], &frames[1].encode()),
        Outcome::Accepted { .. }
    ));
}

#[test]
fn rule_6_an_accepted_frame_again_is_a_repeat_even_with_a_damaged_signature() {
    let frames = chain(&[S, S]);
    for f in &frames {
        let seq = f.entry.seq;
        assert_eq!(after(&frames, &f.encode()), Outcome::Repeat { seq });
        let mut sig_damaged = f.encode();
        sig_damaged[139] ^= 1;
        assert_eq!(after(&frames, &sig_damaged), Outcome::Repeat { seq });
    }
}

#[test]
fn rule_7_a_second_signed_entry_for_a_seq_is_a_fork() {
    let frames = chain(&[S, S, S, S]);
    for seq in [0, 2] {
        let mut other = frames[seq].entry;
        other.segment_root = [0xee; 32];
        let mut receiver = fed(&frames);
        let fork = aborted(seq as u64, Fault::Fork);
        assert_eq!(receiver.frame(&frame(other).encode()), fork);
        // The abort sticks, for every record.
        assert_eq!(receiver.frame(&frames[0].encode()), fork);
        assert_eq!(receiver.close(&close_of(&frames).encode()), fork);
        let reason = Fault::Fork;
        assert_eq!(
            receiver.status(),
            Status::Aborted {
                seq: seq as u64,
                reason
            }
        );
    }
    let mut unsigned = frames[1];
    unsigned.entry.len = 5;
    assert_eq!(
        after(&frames, &unsigned.encode()),
        Outcome::Refused(Fault::Signature)
    );
}

#[test]
fn rule_8_a_wrong_prev_entry_hash_aborts() {
    let frames = chain(&[S, S]);
    assert_eq!(
        after(&[], &frame(entry(0, [0; 32], S)).encode()),
        aborted(0, Fault::PrevHash)
    );
    // The hash of the whole LogOpen, signature included, is not the hash of the LogOpen.
    let whole = *blake3::hash(&open().encode()).as_bytes();
    assert_eq!(
        after(&[], &frame(entry(0, whole, S)).encode()),
        aborted(0, Fault::PrevHash)
    );
    let skips = frame(entry(1, open().hash(), S));
    assert_eq!(
        after(&frames[..1], &skips.encode()),
        aborted(1, Fault::PrevHash)
    );
}

#[test]
fn rule_9_len_is_from_1_to_segment_bytes() {
    let head = open().hash();
    for len in [1, S - 1, S] {
        assert!(matches!(
            after(&[], &frame(entry(0, head, len)).encode()),
            Outcome::Accepted { .. }
        ));
    }
    for len in [0, S + 1, u32::MAX] {
        assert_eq!(
            after(&[], &frame(entry(0, head, len)).encode()),
            aborted(0, Fault::Len)
        );
    }
}

#[test]
fn rule_10_an_entry_after_a_short_one_aborts() {
    let frames = chain(&[S, 10, S]);
    assert_eq!(
        after(&frames[..2], &frames[2].encode()),
        aborted(2, Fault::AfterShort)
    );
    let tiny = chain(&[S, 10, 10]);
    assert_eq!(
        after(&tiny[..2], &tiny[2].encode()),
        aborted(2, Fault::AfterShort)
    );
}

#[test]
fn rule_11_an_entry_after_the_close_aborts() {
    let frames = chain(&[S, S, S]);
    let mut receiver = fed(&frames[..2]);
    assert!(matches!(
        receiver.close(&close_of(&frames[..2]).encode()),
        Outcome::Closed { .. }
    ));
    assert_eq!(
        receiver.frame(&frames[2].encode()),
        aborted(2, Fault::AfterClose)
    );
}
