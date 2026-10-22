mod props;

use super::{BLOCK_BYTES, ByteRange, ByteSize};
use crate::Error;

fn size(text: &str) -> u64 {
    ByteSize::parse(text)
        .unwrap_or_else(|e| panic!("{text}: {e}"))
        .bytes()
}

/// The text fails with a policy error that quotes it and says `why`.
fn refused(text: &str, why: &str) {
    let Err(Error::Policy(message)) = ByteRange::parse(text) else {
        panic!("{text:?} was not refused");
    };
    assert!(message.contains(&format!("{text:?}")), "{message}");
    assert!(message.contains(why), "{text:?}: {why} not in {message}");
}

#[test]
fn every_unit_in_every_casing() {
    for (power, letter) in ["k", "m", "g", "t", "p"].iter().enumerate() {
        let power = power as u32 + 1;
        let binary = 1024u64.pow(power);
        let decimal = 1000u64.pow(power);
        let upper = letter.to_uppercase();
        for unit in [
            letter.to_string(),
            upper.clone(),
            format!("{letter}ib"),
            format!("{upper}iB"),
            format!("{upper}IB"),
            format!("{letter}Ib"),
        ] {
            assert_eq!(size(&format!("3{unit}")), 3 * binary, "{unit}");
        }
        for unit in [
            format!("{letter}b"),
            format!("{upper}B"),
            format!("{letter}B"),
            format!("{upper}b"),
        ] {
            assert_eq!(size(&format!("3{unit}")), 3 * decimal, "{unit}");
        }
    }
    assert_eq!(size("7"), 7);
    assert_eq!(size("7b"), 7);
    assert_eq!(size("7B"), 7);
    assert_eq!(size("0"), 0);
    assert_eq!(size("007K"), 7 * 1024);
}

#[test]
fn a_decimal_is_its_exact_value_rounded_down() {
    let cases = [
        ("4.1G", 4_402_341_478),
        ("4.1GiB", 4_402_341_478),
        ("4.1GB", 4_100_000_000),
        ("1.5", 1),
        ("1.9999B", 1),
        ("0.5K", 512),
        ("0.001K", 1),
        ("0.0009K", 0),
        ("2.5KB", 2500),
        ("1.1P", 1_238_489_897_526_886),
        ("0.33333333333333333333333333333G", 357_913_941),
        ("1.0000000000000000000000000000000000000000000000001K", 1024),
    ];
    for (text, bytes) in cases {
        assert_eq!(size(text), bytes, "{text}");
    }
}

#[test]
fn u64_max_is_the_largest_size_and_one_more_is_refused() {
    assert_eq!(size("18446744073709551615"), u64::MAX);
    assert_eq!(size("18446744073709551615B"), u64::MAX);
    assert_eq!(size("16383.9999999999999999999999P"), u64::MAX);
    for over in [
        "18446744073709551616",
        "16384P",
        "18446744073709551615K",
        "99999999999999999999999",
    ] {
        assert!(
            matches!(ByteSize::parse(over), Err(Error::Policy(m)) if m.contains("u64::MAX")),
            "{over}"
        );
    }
    refused("1:+18446744073709551615", "past u64::MAX");
    assert_eq!(
        ByteRange::parse("0:+18446744073709551615")
            .expect("fits")
            .end(),
        Some(u64::MAX)
    );
}

#[test]
fn every_refusal_quotes_the_text_and_says_why() {
    refused("", "colon");
    refused("100", "colon");
    refused(":", "neither");
    refused("5:5", "empty");
    refused("5:+0", "empty");
    refused("1K:1000", "starts after its end");
    refused(":0", "empty");
    for sign in ["+1:", "-1:", ":-5", ":+5", "1:+-1", "1:++1"] {
        refused(sign, "sign");
    }
    for space in [" 1:", "1 :", "1: 2", "1:2 ", "1 K:", "1:\t2", "1:2\u{a0}"] {
        refused(space, "spaces");
    }
    for empty in [".5:", "5.:", "K:", "1:.", ".:", "1:G", "1.2.3:"] {
        refused(empty, "digits");
    }
    for unit in [
        "1x:",
        "1KiBB:",
        "1kibi:",
        "1e3:",
        "1E:",
        "1:2:3",
        "0x10:",
        "1_000:",
        "1\u{663}:",
    ] {
        refused(unit, "unit");
    }
}

#[test]
fn every_range_form() {
    let cases = [
        ("10:20", 10, Some(20)),
        ("10:", 10, None),
        (":20", 0, Some(20)),
        ("10:+5", 10, Some(15)),
        ("1K:+1K", 1024, Some(2048)),
        ("4G:4.1G", 4 << 30, Some(4_402_341_478)),
        ("0:", 0, None),
    ];
    for (text, start, end) in cases {
        let range = ByteRange::parse(text).expect(text);
        assert_eq!((range.start(), range.end()), (start, end), "{text}");
    }
}

#[test]
fn an_end_past_the_file_is_cut_and_a_start_past_it_is_refused() {
    let range = ByteRange::parse("100:1000").expect("range");
    let resolved = range.resolve(500).expect("cut");
    assert_eq!(
        (resolved.bytes(), resolved.cut_from()),
        (100..500, Some(1000))
    );
    let exact = range.resolve(1000).expect("exact");
    assert_eq!((exact.bytes(), exact.cut_from()), (100..1000, None));
    let open = ByteRange::parse("100:")
        .expect("range")
        .resolve(500)
        .expect("open");
    assert_eq!((open.bytes(), open.cut_from()), (100..500, None));
    for size in [0, 50, 100] {
        let Err(Error::Policy(message)) = range.resolve(size) else {
            panic!("a start at or past {size} bytes was not refused");
        };
        assert!(message.contains(&format!("{size} bytes")), "{message}");
    }
}

/// The plan for `text` in a file of `size` bytes: blocks, skip and keep.
fn plan(text: &str, size: u64) -> (std::ops::Range<u64>, u64, u64) {
    let plan = ByteRange::parse(text)
        .expect(text)
        .resolve(size)
        .expect(text)
        .plan();
    (plan.blocks(), plan.skip(), plan.keep())
}

#[test]
fn plans_on_and_off_the_block_boundaries() {
    let mib = 1 << 20;
    assert_eq!(BLOCK_BYTES, 16_384);
    // Starts and ends on a boundary: exactly one block.
    assert_eq!(plan("16384:32768", mib), (1..2, 0, 16_384));
    // Inside one block.
    assert_eq!(plan("100:200", mib), (0..1, 100, 100));
    // One byte on each side of a boundary: two blocks.
    assert_eq!(plan("16383:16385", mib), (0..2, 16_383, 2));
    // From a boundary to one past the next.
    assert_eq!(plan("16384:32769", mib), (1..3, 0, 16_385));
    // The first byte and the last byte of the file.
    assert_eq!(plan(":1", mib), (0..1, 0, 1));
    assert_eq!(plan("1048575:", mib), (63..64, 16_383, 1));
}

#[test]
fn the_last_block_of_a_file_can_be_short() {
    let plan = ByteRange::parse("39000:")
        .expect("range")
        .resolve(40_000)
        .expect("resolve")
        .plan();
    assert_eq!(plan.blocks(), 2..3);
    assert_eq!(plan.fetched(), 32_768..40_000);
    assert_eq!((plan.skip(), plan.keep()), (39_000 - 32_768, 1000));
    // A file smaller than one block.
    let tiny = ByteRange::parse(":5")
        .expect("range")
        .resolve(10)
        .expect("resolve")
        .plan();
    assert_eq!((tiny.blocks(), tiny.fetched()), (0..1, 0..10));
}

#[test]
fn a_range_that_ends_at_u64_max_plans_without_overflow() {
    let plan = ByteRange::parse("18446744073709551614:")
        .expect("range")
        .resolve(u64::MAX)
        .expect("resolve")
        .plan();
    assert_eq!(plan.keep(), 1);
    assert_eq!(plan.fetched().end, u64::MAX);
}
