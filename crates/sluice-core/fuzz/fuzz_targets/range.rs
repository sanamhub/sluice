#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_core::range::{BLOCK_BYTES, ByteRange};

/// File sizes to resolve a range against when the input names none: one byte, one block, a file
/// with a short last block, and the largest size there is.
const SIZES: [u64; 4] = [1, BLOCK_BYTES, 40_000, u64::MAX];

// The input is the text of a range, and after a zero byte, if there is one, up to 8 bytes of a
// file size, little-endian. Without the zero byte it is resolved against each of `SIZES`, so a
// range as a person writes it is a seed as it is. Whatever parses and resolves must keep the
// parsed start, cut the parsed end to the size and say so, and give a plan that fetches the whole
// range, no block outside it, and skips and keeps only fetched bytes.
fuzz_target!(|data: &[u8]| {
    let (text, sizes) = match data.iter().position(|&b| b == 0) {
        Some(at) => {
            let mut size = [0u8; 8];
            let tail = &data[at + 1..];
            let n = tail.len().min(8);
            size[..n].copy_from_slice(&tail[..n]);
            (&data[..at], vec![u64::from_le_bytes(size)])
        }
        None => (data, SIZES.to_vec()),
    };
    let Ok(text) = std::str::from_utf8(text) else {
        return;
    };
    let Ok(range) = ByteRange::parse(text) else {
        return;
    };
    if let Some(end) = range.end() {
        assert!(
            range.start() < end,
            "an empty or backward range parsed: {text:?}"
        );
    }
    for size in sizes {
        let Ok(resolved) = range.resolve(size) else {
            assert!(
                range.start() >= size,
                "refused a range that starts in the file"
            );
            continue;
        };
        let bytes = resolved.bytes();
        assert!(
            bytes.start < bytes.end && bytes.end <= size,
            "out of the file"
        );
        assert_eq!(bytes.start, range.start(), "the start moved");
        // `a:` runs to the end of the file, and any other end is cut to the size.
        let end = range.end().map_or(size, |end| end.min(size));
        assert_eq!(bytes.end, end, "the end is not the parsed end cut to the size");
        assert_eq!(
            resolved.cut_from(),
            range.end().filter(|&end| end > size),
            "cut_from does not say whether the end was cut"
        );
        let plan = resolved.plan();
        let fetched = plan.fetched();
        assert!(
            fetched.start <= bytes.start && bytes.end <= fetched.end,
            "not covered"
        );
        assert_eq!(
            plan.blocks().start,
            bytes.start / BLOCK_BYTES,
            "a block before the range"
        );
        assert_eq!(
            plan.blocks().end - 1,
            (bytes.end - 1) / BLOCK_BYTES,
            "a block after it"
        );
        assert_eq!(
            fetched.start + plan.skip(),
            bytes.start,
            "skips the wrong bytes"
        );
        assert_eq!(
            plan.keep(),
            bytes.end - bytes.start,
            "keeps the wrong bytes"
        );
    }
});
