#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_delta::{MAX_ENTRIES, NeedRanges};

// The input is the bytes of a need-ranges message as a peer could send them. A decoder that accepts
// bytes accepts one spelling of them, and what it read is a set the spec allows: ranges that are
// not empty, ascending, apart from each other, ending inside a u64, and no more of them than a
// message may carry. The wanted bytes are their sum. A refusal does not panic.
fuzz_target!(|data: &[u8]| {
    let Ok(need) = NeedRanges::decode(data) else {
        return;
    };
    assert_eq!(need.encode(), data, "one set, one spelling");
    assert!(need.ranges().len() <= MAX_ENTRIES as usize);
    let mut end = None::<u64>;
    let mut wanted = 0u64;
    for range in need.ranges() {
        assert!(range.len() > 0);
        if let Some(end) = end {
            assert!(range.start() > end, "ranges touch or overlap");
        }
        end = Some(range.end());
        wanted += range.len();
    }
    assert_eq!(need.wanted(), wanted);
});
