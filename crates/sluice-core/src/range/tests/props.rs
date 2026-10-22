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

/// A file size of at least 1 byte, a start inside it, and an end past it.
fn past_the_end() -> impl Strategy<Value = (u64, u64, u64)> {
    prop_oneof![1u64..200_000, 1u64..u64::MAX]
        .prop_flat_map(|size| (Just(size), 0..size, size + 1..=u64::MAX))
}

/// A decimal with 1 to 18 fraction digits, leading zeros included, and a unit.
fn decimal() -> impl Strategy<Value = (u64, String, (String, u64))> {
    let integer = prop_oneof![0u64..1000, 0..=u64::from(u32::MAX), any::<u64>()];
    (integer, "[0-9]{1,18}", unit())
}

/// What `integer.fraction` times `unit` is, rounded down, worked out in `u128` from the definition
/// and not from the parser's carry: `(integer * 10^k + fraction) * unit / 10^k`. A product past
/// `u128` is far past `u64::MAX`, because `10^k` is at most `10^18`.
fn oracle(integer: u64, fraction: &str, unit: u64) -> Option<u64> {
    let k = u32::try_from(fraction.len()).expect("short");
    let scale = 10u128.pow(k);
    let digits: u128 = fraction.parse().expect("digits");
    let exact = u128::from(integer)
        .checked_mul(scale)?
        .checked_add(digits)?
        .checked_mul(u128::from(unit))?;
    u64::try_from(exact / scale).ok()
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
    fn a_decimal_is_its_exact_value_rounded_down((integer, fraction, (unit, bytes)) in decimal()) {
        let text = format!("{integer}.{fraction}{unit}");
        let got = ByteSize::parse(&text).ok().map(ByteSize::bytes);
        prop_assert_eq!(got, oracle(integer, &fraction, bytes), "{}", text);
    }

    #[test]
    fn an_end_past_the_file_is_cut_and_says_where_it_was((size, start, end) in past_the_end()) {
        for text in [format!("{start}:{end}"), format!("{start}:+{}", end - start)] {
            let range = ByteRange::parse(&text).expect("a range");
            let resolved = range.resolve(size).expect("starts in the file");
            prop_assert_eq!(resolved.bytes(), start..size, "{}", text);
            prop_assert_eq!(resolved.cut_from(), Some(end), "{}", text);
            prop_assert_eq!(resolved.plan().keep(), size - start, "{}", text);
        }
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
