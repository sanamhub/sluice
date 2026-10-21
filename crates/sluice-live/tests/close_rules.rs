//! Rules 1, 3 and 12 to 15 for the close, and what a closed receiver answers.

mod common;

use common::*;
use sluice_live::{Fault, Outcome, Receiver, Status};

fn closed(total_len: u64) -> Outcome {
    Outcome::Closed {
        final_root: [0xf0; 32],
        total_len,
    }
}

fn aborted(seq: u64, reason: Fault) -> Outcome {
    Outcome::Aborted { seq, reason }
}

/// A receiver that took `frames` and the close of them.
fn closed_after(frames: &[sluice_live::Frame]) -> Receiver {
    let mut receiver = fed(frames);
    let total = frames.iter().map(|f| u64::from(f.entry.len)).sum();
    assert_eq!(receiver.close(&close_of(frames).encode()), closed(total));
    receiver
}

#[test]
fn a_close_after_a_short_or_a_full_last_segment_closes_the_stream() {
    for lens in [&[S, S, 9][..], &[S, S], &[1]] {
        let receiver = closed_after(&chain(lens));
        let total_len = lens.iter().map(|&l| u64::from(l)).sum();
        let final_root = [0xf0; 32];
        assert_eq!(
            receiver.status(),
            Status::Closed {
                final_root,
                total_len
            }
        );
    }
}

#[test]
fn an_empty_stream_closes_on_the_hash_of_the_open() {
    let mut receiver = receiver();
    assert_eq!(receiver.close(&close(0, open().hash()).encode()), closed(0));
}

#[test]
fn rule_1_and_3_a_close_that_does_not_verify_is_refused() {
    let frames = chain(&[S, 3]);
    let good = close_of(&frames).encode();
    let mut receiver = fed(&frames);
    let before = receiver.clone();
    assert_eq!(
        receiver.close(&good[..143]),
        Outcome::Refused(Fault::Length)
    );
    for at in [0, 32, 40, 48, 79, 80, 143] {
        let mut bytes = good;
        bytes[at] ^= 1;
        assert_eq!(
            receiver.close(&bytes),
            Outcome::Refused(Fault::Signature),
            "{at}"
        );
    }
    let mut entry_label = close_of(&frames);
    entry_label.sig = sign(ENTRY, &STREAM, &good[..80]);
    assert_eq!(
        receiver.close(&entry_label.encode()),
        Outcome::Refused(Fault::Signature)
    );
    assert_eq!(receiver, before);
    assert_eq!(receiver.close(&good), closed(S as u64 + 3));
}

#[test]
fn rule_12_a_close_ahead_of_the_entries_is_refused_and_counts_once_they_arrive() {
    let frames = chain(&[S, S, 4]);
    let bytes = close_of(&frames).encode();
    let mut receiver = receiver();
    for f in &frames {
        let before = receiver.clone();
        assert_eq!(receiver.close(&bytes), Outcome::Refused(Fault::CloseAhead));
        assert_eq!(receiver, before);
        receiver.frame(&f.encode());
    }
    assert_eq!(receiver.close(&bytes), closed(2 * S as u64 + 4));
}

#[test]
fn rule_13_a_total_len_that_is_not_the_sum_aborts() {
    let frames = chain(&[S, S]);
    let head = frames[1].entry.hash();
    let total = 2 * u64::from(S);
    assert_eq!(
        fed(&frames).close(&close(total - 1, head).encode()),
        aborted(2, Fault::TotalLen)
    );
    // After a short entry no more can come, so a larger total is the sender's.
    let short = chain(&[S, 5]);
    let head = short[1].entry.hash();
    let more = close(u64::from(S) + 6, head);
    assert_eq!(
        fed(&short).close(&more.encode()),
        aborted(2, Fault::TotalLen)
    );
}

#[test]
fn rule_14_a_last_entry_hash_that_is_not_the_head_aborts() {
    let frames = chain(&[S, S]);
    let total = 2 * u64::from(S);
    let wrong = close(total, frames[0].entry.hash());
    assert_eq!(
        fed(&frames).close(&wrong.encode()),
        aborted(2, Fault::LastEntry)
    );
    assert_eq!(
        receiver().close(&close(0, [0; 32]).encode()),
        aborted(0, Fault::LastEntry)
    );
}

#[test]
fn rule_15_a_second_signed_close_with_other_bytes_aborts() {
    let frames = chain(&[S, 8]);
    let mut receiver = closed_after(&frames);
    let mut other = close_of(&frames);
    other.final_root = [0x0f; 32];
    other.sig = sign(CLOSE, &STREAM, &other.encode()[..80]);
    assert_eq!(
        receiver.close(&other.encode()),
        aborted(2, Fault::SecondClose)
    );
}

#[test]
fn a_closed_stream_answers_with_the_close_unless_the_sender_broke_it() {
    let frames = chain(&[S, 8]);
    let mut receiver = closed_after(&frames);
    let total = u64::from(S) + 8;
    let mut damaged = close_of(&frames).encode();
    damaged[90] ^= 1;
    assert_eq!(receiver.close(&close_of(&frames).encode()), closed(total));
    assert_eq!(receiver.close(&damaged), closed(total));
    assert_eq!(receiver.frame(&frames[0].encode()), closed(total));
    assert_eq!(receiver.frame(&[0; 3]), closed(total));
    let mut fork = frames[1].entry;
    fork.len = 9;
    assert_eq!(
        receiver.frame(&frame(fork).encode()),
        aborted(1, Fault::Fork)
    );
}
