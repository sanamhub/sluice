use proptest::prelude::*;

use super::super::{BLOCK_BYTES, ByteRange, ByteSize};

/// A unit as a person may write it, in a random casing, and its size in bytes.
fn unit() -> impl Strategy<Value = (String, u64)> {
    let forms = prop_oneof![
        Just(("", 1u64)),
        Just(("b", 1)),
        Just(("k", 1 << 10)),
        Just(("kib", 1 << 10)),
        Just(("kb", 1000)),
        Just(("m", 1 << 20)),
        Just(("mib", 1 << 20)),
        Just(("mb", 1_000_000)),
        Just(("g", 1 << 30)),
        Just(("gb", 1_000_000_000)),
        Just(("t", 1 << 40)),
        Just(("tb", 1_000_000_000_000)),
        Just(("p", 1 << 50)),
        Just(("pib", 1 << 50)),
        Just(("pb", 1_000_000_000_000_000)),
    ];
    (forms, any::<u8>()).prop_map(|((text, bytes), casing)| {
        let text: String = text
            .chars()
            .enumerate()
            .map(|(at, c)| {
                if casing & (1 << at) != 0 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        (text, bytes)
    })
}

/// A resolved range: a file size from 1 byte, and a start and length inside it.
fn resolved() -> impl Strategy<Value = (u64, u64, u64)> {
    prop_oneof![1u64..200_000, 1u64..=u64::MAX].prop_flat_map(|size| {
        (Just(size), 0..size)
            .prop_flat_map(|(size, start)| (Just(size), Just(start), 1..=size - start))
    })
}

proptest! {
    #[test]
    fn any_formatted_size_parses_back_to_itself(bytes in any::<u64>()) {
        let text = ByteSize(bytes).to_string();
        prop_assert_eq!(ByteSize::parse(&text).expect("parses").bytes(), bytes, "{}", text);
    }

    #[test]
    fn a_number_and_a_unit_in_any_casing_is_their_product(
        number in any::<u32>(),
        (unit, bytes) in unit(),
    ) {
        let text = format!("{number}{unit}");
        let want = u64::from(number).checked_mul(bytes);
        let got = ByteSize::parse(&text).ok().map(ByteSize::bytes);
        prop_assert_eq!(got, want, "{}", text);
    }

    #[test]
    fn the_plan_covers_the_range_and_no_block_outside_it((size, start, len) in resolved()) {
        let range = ByteRange::parse(&format!("{start}:+{len}")).expect("a range");
        let resolved = range.resolve(size).expect("inside the file");
        let plan = resolved.plan();
        let (fetched, end) = (plan.fetched(), start + len);
        // Covers the range.
        prop_assert!(fetched.start <= start && end <= fetched.end);
        // The first block holds the first byte, and the last block holds the last byte.
        prop_assert_eq!(plan.blocks().start, start / BLOCK_BYTES);
        prop_assert_eq!(plan.blocks().end - 1, (end - 1) / BLOCK_BYTES);
        // Skip and keep land exactly on the range, inside what was fetched.
        prop_assert!(plan.skip() + plan.keep() <= fetched.end - fetched.start);
        prop_assert_eq!(fetched.start + plan.skip(), start);
        prop_assert_eq!(plan.keep(), len);
        prop_assert!(plan.skip() < BLOCK_BYTES);
    }
}
