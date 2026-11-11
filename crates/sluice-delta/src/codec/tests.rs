//! Both messages are read from a peer, so every test here is about a message that must be refused
//! before it is believed, and about the one encoding each value has.

use super::{Entry, MAX_ENTRIES, NeedRanges, Range, SigList, varint};
use crate::Error;

fn strong(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn list() -> SigList {
    SigList::new(
        8192,
        vec![
            Entry::new(2048, strong(1)).unwrap(),
            Entry::new(9000, strong(2)).unwrap(),
        ],
    )
    .unwrap()
}

/// A list is the algorithm, the average, a count and 20 bytes per entry.
#[test]
fn a_sig_list_round_trips_and_says_how_much_it_covers() {
    let list = list();
    let bytes = list.encode();
    assert_eq!(bytes[0], SigList::ALGO);
    assert_eq!(bytes.len(), 5 + 1 + 2 * 20);
    assert_eq!(SigList::decode(&bytes).unwrap(), list);
    assert_eq!(list.total_len(), 2048 + 9000);
    assert_eq!(list.avg_bytes(), 8192);
}

#[test]
fn a_sig_list_refuses_an_algorithm_it_does_not_read() {
    let mut bytes = list().encode();
    bytes[0] = 2;
    assert_eq!(SigList::decode(&bytes), Err(Error::Algorithm(2)));
}

/// The average has to be one the chunker can cut with, or the list cannot have come from one.
#[test]
fn a_sig_list_refuses_an_average_outside_its_bounds() {
    let mut bytes = list().encode();
    bytes[1..5].copy_from_slice(&3000u32.to_le_bytes());
    assert_eq!(SigList::decode(&bytes), Err(Error::Average));
}

/// A chunk longer than the parameters allow cannot have been cut by them.
#[test]
fn a_sig_list_refuses_a_chunk_longer_than_the_parameters_allow() {
    assert_eq!(
        SigList::new(8192, vec![Entry::new(65537, strong(1)).unwrap()]),
        Err(Error::ChunkLength)
    );
    assert_eq!(
        SigList::new(8192, vec![Entry::new(65536, strong(1)).unwrap()])
            .unwrap()
            .entries()
            .len(),
        1
    );
}

#[test]
fn an_entry_of_no_bytes_is_refused() {
    assert_eq!(Entry::new(0, strong(1)), Err(Error::ChunkLength));
}

/// The count is read first and bounded before anything is reserved, so a peer cannot ask for a
/// gigabyte with five bytes.
#[test]
fn a_sig_list_refuses_a_count_past_the_limit_before_it_allocates() {
    let mut bytes = vec![SigList::ALGO];
    bytes.extend_from_slice(&8192u32.to_le_bytes());
    bytes.extend_from_slice(&varint(u64::from(MAX_ENTRIES) + 1));
    assert_eq!(SigList::decode(&bytes), Err(Error::TooManyEntries));
}

#[test]
fn a_sig_list_refuses_a_message_that_ends_early_or_late() {
    let bytes = list().encode();
    assert_eq!(
        SigList::decode(&bytes[..bytes.len() - 1]),
        Err(Error::Truncated)
    );
    let mut late = bytes.clone();
    late.push(0);
    assert_eq!(SigList::decode(&late), Err(Error::Trailing));
    assert_eq!(SigList::decode(&[]), Err(Error::Truncated));
}

/// A count written in more bytes than it needs would give one list two spellings, and a varint past
/// ten bytes is not a count at all. Neither is a truncated message: both have the bytes, and neither
/// is something an encoder writes.
#[test]
fn a_count_that_is_not_the_shortest_spelling_is_refused() {
    let mut bytes = vec![SigList::ALGO];
    bytes.extend_from_slice(&8192u32.to_le_bytes());
    bytes.extend_from_slice(&[0x81, 0x00]);
    assert_eq!(SigList::decode(&bytes), Err(Error::Varint));

    let mut long = vec![SigList::ALGO];
    long.extend_from_slice(&8192u32.to_le_bytes());
    long.extend_from_slice(&[0x80; 9]);
    long.push(0x02);
    assert_eq!(SigList::decode(&long), Err(Error::Varint));
}

fn need() -> NeedRanges {
    NeedRanges::new(
        [9; 32],
        vec![
            Range::new(0, 1024).unwrap(),
            Range::new(4096, 2048).unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn need_ranges_round_trip_and_count_their_bytes() {
    let need = need();
    let bytes = need.encode();
    assert_eq!(bytes.len(), 32 + 1 + 2 * 16);
    assert_eq!(NeedRanges::decode(&bytes).unwrap(), need);
    assert_eq!(need.wanted(), 1024 + 2048);
    assert_eq!(*need.root(), [9; 32]);
}

#[test]
fn ranges_must_be_ascending_and_apart() {
    let root = [9; 32];
    for ranges in [
        vec![Range::new(10, 10).unwrap(), Range::new(5, 5).unwrap()],
        vec![Range::new(10, 10).unwrap(), Range::new(15, 5).unwrap()],
        vec![Range::new(10, 10).unwrap(), Range::new(10, 5).unwrap()],
        vec![Range::new(10, 10).unwrap(), Range::new(19, 1).unwrap()],
        vec![Range::new(10, 10).unwrap(), Range::new(20, 5).unwrap()],
    ] {
        assert_eq!(NeedRanges::new(root, ranges), Err(Error::Ranges));
    }
    assert!(
        NeedRanges::new(
            root,
            vec![Range::new(10, 10).unwrap(), Range::new(21, 1).unwrap()]
        )
        .is_ok()
    );
}

/// An empty range asks for nothing, and a range that runs past a `u64` cannot name bytes.
#[test]
fn an_empty_range_and_one_past_the_end_of_the_offset_space_are_refused() {
    assert_eq!(Range::new(5, 0), Err(Error::Ranges));
    assert_eq!(Range::new(u64::MAX, 1), Err(Error::Overflow));
    assert_eq!(Range::new(u64::MAX - 1, 1).unwrap().end(), u64::MAX);
}

#[test]
fn need_ranges_refuse_a_message_that_ends_early_or_late() {
    let bytes = need().encode();
    assert_eq!(
        NeedRanges::decode(&bytes[..bytes.len() - 1]),
        Err(Error::Truncated)
    );
    let mut late = bytes.clone();
    late.push(0);
    assert_eq!(NeedRanges::decode(&late), Err(Error::Trailing));
}

/// Ranges are checked after they are read too, so a peer cannot send them out of order.
#[test]
fn need_ranges_refuse_ranges_out_of_order_on_the_wire() {
    let mut bytes = vec![9; 32];
    bytes.extend_from_slice(&varint(2));
    for (start, len) in [(4096u64, 10u64), (0, 10)] {
        bytes.extend_from_slice(&start.to_le_bytes());
        bytes.extend_from_slice(&len.to_le_bytes());
    }
    assert_eq!(NeedRanges::decode(&bytes), Err(Error::Ranges));
}
