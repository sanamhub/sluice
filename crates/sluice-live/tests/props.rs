//! Properties of the chain: what the sender builds is accepted whole, a damaged frame never counts,
//! a replay is a repeat, and providers that drop, repeat and damage frames end where one clean
//! provider would.

mod common;

use common::*;
use proptest::prelude::*;
use sluice_live::{Frame, Outcome, Receiver, Sender, Status};

/// A small segment size keeps the lengths in reach. The chain rules do not depend on it.
fn built(full: usize, last: u32) -> (Receiver, Vec<Frame>, sluice_live::LogClose) {
    let mut sender = Sender::new(key(), STREAM, S, 0).expect("valid");
    let receiver = Receiver::new(STREAM, public(), &sender.log_open().encode()).expect("valid");
    let mut frames: Vec<Frame> = (0..full)
        .map(|at| sender.seal([at as u8; 32], S).expect("full"))
        .collect();
    if last > 0 {
        frames.push(sender.seal([0xaa; 32], last).expect("the last"));
    }
    let close = sender.close([0xcc; 32], 0);
    (receiver, frames, close)
}

fn accepted(outcome: Outcome) -> Option<u64> {
    match outcome {
        Outcome::Accepted { seq, .. } => Some(seq),
        _ => None,
    }
}

proptest! {
    // Every case signs and verifies tens of records, which an unoptimised Ed25519 does in
    // milliseconds each, so the cases are fewer than the default 256.
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_chain_the_sender_builds_is_accepted_and_closes_with_its_length(
        full in 0..40usize,
        last in prop_oneof![Just(0), 1..=S],
    ) {
        let (mut receiver, frames, close) = built(full, last);
        for (seq, f) in frames.iter().enumerate() {
            prop_assert_eq!(accepted(receiver.frame(&f.encode())), Some(seq as u64));
        }
        let total_len = full as u64 * u64::from(S) + u64::from(last);
        let final_root = [0xcc; 32];
        prop_assert_eq!(receiver.close(&close.encode()), Outcome::Closed { final_root, total_len });
    }

    #[test]
    fn a_frame_with_one_byte_changed_never_counts_as_another_entry(
        full in 1..8usize,
        taken in any::<prop::sample::Index>(),
        target in any::<prop::sample::Index>(),
        at in 0..Frame::LEN,
        flip in 1..=255u8,
    ) {
        let (mut receiver, frames, _) = built(full, 0);
        let taken = taken.index(frames.len() + 1);
        for f in &frames[..taken] {
            receiver.frame(&f.encode());
        }
        let target = target.index(frames.len());
        let mut bytes = frames[target].encode();
        bytes[at] ^= flip;
        let before = receiver.clone();
        match receiver.frame(&bytes) {
            Outcome::Refused(_) => prop_assert_eq!(&receiver, &before),
            // An accepted entry with a damaged signature is still that entry.
            Outcome::Repeat { seq } => {
                prop_assert!(at >= 76 && seq == target as u64 && target < taken);
            }
            other => prop_assert!(false, "{other:?} from a damaged frame"),
        }
        // Whatever the damage was, the true frame still counts in its place.
        if target == taken {
            let again = receiver.frame(&frames[target].encode());
            prop_assert_eq!(accepted(again), Some(target as u64));
        }
    }

    #[test]
    fn replaying_an_accepted_frame_is_a_repeat(
        full in 1..12usize,
        pick in any::<prop::sample::Index>(),
    ) {
        let (mut receiver, frames, _) = built(full, 3);
        for f in &frames {
            receiver.frame(&f.encode());
        }
        let before = receiver.clone();
        let seq = pick.index(frames.len());
        prop_assert_eq!(receiver.frame(&frames[seq].encode()), Outcome::Repeat { seq: seq as u64 });
        prop_assert_eq!(receiver, before);
    }

    #[test]
    fn several_providers_with_drops_repeats_and_damage_end_in_the_clean_chain(
        full in 0..10usize,
        last in 1..=S,
        events in prop::collection::vec((0..12usize, 0..4u8, 0..Frame::LEN, 1..=255u8), 0..80),
    ) {
        let (mut receiver, frames, close) = built(full, last);
        let mut got = Vec::new();
        let mut deliver = |receiver: &mut Receiver, bytes: &[u8]| {
            match receiver.frame(bytes) {
                Outcome::Accepted { seq, segment_root, len } => got.push((seq, segment_root, len)),
                Outcome::Aborted { .. } => panic!("a provider aborted the stream"),
                _ => {}
            }
        };
        for (pick, kind, at, flip) in events {
            let Some(f) = frames.get(pick % (frames.len() + 1)) else {
                // Some providers send the close early.
                let outcome = receiver.close(&close.encode());
                let aborted = matches!(outcome, Outcome::Aborted { .. });
                prop_assert!(!aborted, "the real close aborted the stream");
                continue;
            };
            let mut bytes = f.encode();
            match kind {
                0 => continue,
                1 => bytes[at] ^= flip,
                2 => deliver(&mut receiver, &bytes),
                _ => {}
            }
            deliver(&mut receiver, &bytes);
        }
        // One clean provider then gives the whole chain.
        for f in &frames {
            deliver(&mut receiver, &f.encode());
        }
        let clean: Vec<_> = frames
            .iter()
            .map(|f| (f.entry.seq, f.entry.segment_root, f.entry.len))
            .collect();
        prop_assert_eq!(got, clean);
        let total_len = full as u64 * u64::from(S) + u64::from(last);
        let final_root = [0xcc; 32];
        prop_assert_eq!(receiver.close(&close.encode()), Outcome::Closed { final_root, total_len });
        prop_assert_eq!(receiver.status(), Status::Closed { final_root, total_len });
    }
}
