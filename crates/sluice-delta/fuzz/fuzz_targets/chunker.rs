#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_delta::{Params, StreamCut};

// The first byte picks the average (a power of two from 256 up to 8192, which keeps a chunk small
// enough to reach every rule on short inputs), the second the block a stream is fed in, and the rest
// is the content. The cuts must cover the input exactly, none may be longer than `max` or, but for
// the last, shorter than `min`, and a stream fed in blocks must cut where the whole input is cut.
fuzz_target!(|data: &[u8]| {
    let [exponent, block, content @ ..] = data else {
        return;
    };
    let params = Params::new(256 << (exponent % 6)).expect("an average in range");
    let block = usize::from(*block) + 1;
    let whole = params.boundaries(content);
    let mut at = 0;
    for (index, end) in whole.iter().enumerate() {
        let len = end - at;
        assert!(len >= 1 && len <= params.max() as usize);
        if index + 1 < whole.len() {
            assert!(len >= params.min() as usize, "a chunk before the last is short");
        }
        at = *end;
    }
    assert_eq!(at, content.len(), "the chunks do not cover the input");

    let mut cut = StreamCut::new(params);
    let mut ends = Vec::new();
    let mut total = 0;
    let mut each = |chunk: &[u8]| {
        total += chunk.len();
        ends.push(total);
    };
    for part in content.chunks(block) {
        cut.feed(part, &mut each);
    }
    cut.finish(&mut each);
    assert_eq!(ends, whole, "a stream cuts where the whole input does");
});
