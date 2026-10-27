//! The range arithmetic behind the sender's byte-count fallback: which bytes a request asked for,
//! and whether the union of everything served for one root covers the whole blob. Two requests for
//! the same half are one half, and a request that aborted counts only the bytes it sent.

use iroh_blobs::protocol::ChunkRangesSeq;

/// How many separate ranges one root may be accounted in. A client that asks for one range at a
/// time could otherwise grow the map without bound; past the cap the root is never counted as
/// delivered, which keeps the sender serving rather than stopping early.
pub(super) const MAX_SPANS: usize = 4096;

/// The chunk number a request uses for "the last chunk of this blob", before it knows how large the
/// blob is. It means the blob's final chunk, which is the only part of it such a request can name.
const LAST_CHUNK: u64 = u64::MAX;

/// The size of one `iroh-blobs` chunk, which is what a chunk range counts in.
const CHUNK_BYTES: u64 = 1024;

/// The byte ranges a request asked for, as it asked them. A get names one blob, so the ranges of
/// blob 0 are the ones it serves; a request that names another blob as well is not one a get makes
/// and it is not counted.
pub(super) fn requested(ranges: &ChunkRangesSeq) -> Vec<(u64, u64)> {
    use std::ops::{Bound, RangeBounds};

    let mut out = Vec::new();
    for range in ranges[0].iter() {
        let (start, end) = match (range.start_bound(), range.end_bound()) {
            (Bound::Included(chunk), _) if chunk.0 == LAST_CHUNK => {
                // The last-chunk marker is the whole range, so it has no start below its end.
                out.push((LAST_CHUNK, LAST_CHUNK));
                continue;
            }
            (Bound::Included(chunk), Bound::Excluded(end)) => (chunk.to_bytes(), end.to_bytes()),
            (Bound::Included(chunk), Bound::Unbounded) => (chunk.to_bytes(), u64::MAX),
            (Bound::Included(chunk), Bound::Included(_)) => (
                chunk.to_bytes(),
                chunk.to_bytes().saturating_add(CHUNK_BYTES),
            ),
            _ => continue,
        };
        if start < end {
            out.push((start, end));
        }
    }
    out
}

/// The same ranges clamped to a blob of `size` bytes. The last-chunk marker becomes the blob's
/// final chunk.
pub(super) fn clamp(ranges: &[(u64, u64)], size: u64) -> Vec<(u64, u64)> {
    ranges
        .iter()
        .map(|(start, end)| {
            if *start == LAST_CHUNK {
                (size.saturating_sub(CHUNK_BYTES), size)
            } else {
                ((*start).min(size), (*end).min(size))
            }
        })
        .filter(|(start, end)| start < end)
        .collect()
}

/// The first `sent` bytes of `ranges`, which is what an aborted request got through.
pub(super) fn front(ranges: Vec<(u64, u64)>, sent: u64) -> Vec<(u64, u64)> {
    let mut left = sent;
    let mut out = Vec::new();
    for (start, end) in ranges {
        if left == 0 {
            break;
        }
        let take = (end - start).min(left);
        out.push((start, start + take));
        left -= take;
    }
    out
}

/// Adds one range to a merged, ascending list of them.
pub(super) fn add(spans: &mut Vec<(u64, u64)>, start: u64, end: u64) {
    if start >= end || spans.len() >= MAX_SPANS {
        return;
    }
    spans.push((start, end));
    spans.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for (start, end) in spans.drain(..) {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    *spans = merged;
}

/// Whether the served ranges are exactly the whole blob, in one span.
pub(super) fn covers(spans: &[(u64, u64)], size: u64) -> bool {
    spans == [(0, size)]
}

#[cfg(test)]
mod tests {
    use super::{add, clamp, covers, front, requested};
    use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, ChunkRangesSeq};

    fn ask(ranges: ChunkRanges) -> Vec<(u64, u64)> {
        requested(&ChunkRangesSeq::from_ranges([ranges]))
    }

    fn whole(size: u64) -> Vec<(u64, u64)> {
        clamp(&ask(ChunkRanges::all()), size)
    }

    #[test]
    fn asking_twice_for_the_first_half_is_not_the_whole_file() {
        let half = clamp(&ask(ChunkRanges::bytes(0..65_536)), 131_072);
        let mut spans = Vec::new();
        for _ in 0..2 {
            for (start, end) in half.clone() {
                add(&mut spans, start, end);
            }
        }
        assert!(!covers(&spans, 131_072), "{spans:?}");
    }

    #[test]
    fn the_two_halves_add_up_to_the_whole_file() {
        let mut spans = Vec::new();
        for (start, end) in clamp(&ask(ChunkRanges::bytes(0..65_536)), 131_072) {
            add(&mut spans, start, end);
        }
        for (start, end) in clamp(&ask(ChunkRanges::bytes(65_536..131_072)), 131_072) {
            add(&mut spans, start, end);
        }
        assert!(covers(&spans, 131_072), "{spans:?}");
    }

    #[test]
    fn an_open_range_reaches_the_end_of_the_blob() {
        assert_eq!(whole(4096), vec![(0, 4096)]);
    }

    #[test]
    fn an_aborted_request_counts_what_it_sent() {
        assert_eq!(
            front(vec![(0, 1000), (2000, 3000)], 1500),
            vec![(0, 1000), (2000, 2500)]
        );
        assert_eq!(front(vec![(0, 1000)], 0), Vec::new());
    }

    #[test]
    fn the_last_chunk_of_a_blob_that_does_not_know_its_size_is_its_tail() {
        assert_eq!(clamp(&[requested_last()], 4096), vec![(3072, 4096)]);
    }

    fn requested_last() -> (u64, u64) {
        requested(&ChunkRangesSeq::from_ranges([ChunkRanges::last_chunk()]))[0]
    }
}
