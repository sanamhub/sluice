//! The chunker's rules and the property a delta lives on: a change moves one boundary, not all of
//! them.

use proptest::prelude::*;

use super::{MAX_AVG, MIN_AVG, Params};
use crate::Error;

fn params() -> Params {
    Params::default()
}

#[test]
fn an_average_that_is_not_a_power_of_two_is_refused() {
    for bad in [0, 1, 255, 300, 1000, MIN_AVG + 1, MAX_AVG * 2] {
        assert_eq!(Params::new(bad), Err(Error::Average), "{bad}");
    }
    for good in [MIN_AVG, 1024, 8192, MAX_AVG] {
        assert!(Params::new(good).is_ok(), "{good}");
    }
}

/// The minimum and the maximum are what `SPEC.md` section 7 says they are, so two implementations
/// that read one `avg_bytes` cut at the same places.
#[test]
fn the_bounds_follow_from_the_average() {
    let params = Params::new(8192).unwrap();
    assert_eq!(params.min(), 2048);
    assert_eq!(params.max(), 65536);
    let small = Params::new(256).unwrap();
    assert_eq!(small.min(), 64);
    assert_eq!(small.max(), 2048);
}

#[test]
fn an_empty_input_has_no_chunks_and_a_short_one_is_one_chunk() {
    assert_eq!(params().boundaries(&[]), Vec::<usize>::new());
    assert_eq!(params().boundaries(&[7; 100]), vec![100]);
    assert_eq!(params().chunks(&[7; 100]).count(), 1);
}

proptest! {
    /// Every byte of the input lands in exactly one chunk, in order.
    #[test]
    fn the_chunks_cover_the_input(data in prop::collection::vec(any::<u8>(), 0..40_000)) {
        let chunks: Vec<&[u8]> = params().chunks(&data).collect();
        prop_assert_eq!(chunks.concat(), data);
    }

    /// The same bytes cut at the same places every time, which is the whole of matching.
    #[test]
    fn the_same_bytes_cut_at_the_same_places(data in prop::collection::vec(any::<u8>(), 0..40_000)) {
        prop_assert_eq!(params().boundaries(&data), params().boundaries(&data));
    }

    /// No chunk is shorter than the minimum or longer than the maximum, except the last one, which
    /// is the rest of the input.
    #[test]
    fn every_chunk_stays_within_the_bounds(data in prop::collection::vec(any::<u8>(), 0..80_000)) {
        let params = params();
        let chunks: Vec<&[u8]> = params.chunks(&data).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            if index + 1 == chunks.len() {
                prop_assert!(!chunk.is_empty());
                prop_assert!(chunk.len() <= params.max() as usize);
            } else {
                prop_assert!(chunk.len() >= params.min() as usize);
                prop_assert!(chunk.len() <= params.max() as usize);
            }
        }
    }

    /// The property a delta lives on: one changed byte moves its own boundary and, at most, the one
    /// after it, and the rest of the file cuts as it did. A file of 64 KiB has about eight chunks.
    #[test]
    fn one_changed_byte_moves_at_most_two_boundaries(
        mut data in prop::collection::vec(any::<u8>(), 64 * 1024..64 * 1024 + 1),
        at in 0usize..64 * 1024,
    ) {
        let before = params().boundaries(&data);
        data[at] ^= 0xff;
        let after = params().boundaries(&data);
        let common = before.iter().filter(|end| after.contains(end)).count();
        prop_assert!(
            common + 2 >= before.len().min(after.len()),
            "{} of {} boundaries stayed",
            common,
            before.len()
        );
    }
}
