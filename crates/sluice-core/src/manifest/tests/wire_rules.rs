//! Decoding rules 1 to 6 of `SPEC.md` section 8: the limits, the version and the bytes.

use super::common::*;
use crate::manifest::{Limits, Manifest};

const EMPTY: [u8; 3] = [1, 0, 0];

#[test]
fn rule_1_the_byte_limit_is_checked_before_anything_is_read() {
    let limits = Limits {
        max_bytes: 3,
        max_entries: 10,
    };
    assert!(Manifest::decode(&EMPTY, limits).is_ok());
    // Four bytes that are not even a manifest: the limit answers first.
    let err = Manifest::decode(&[0xff; 4], limits).unwrap_err();
    assert_eq!(rule_of(&err), (1, None));
    let err = Manifest::decode(&[1, 0, 0, 0], limits).unwrap_err();
    assert_eq!(rule_of(&err), (1, None));
    // The same check is there for a caller that has only a size.
    assert!(limits.check_bytes(3).is_ok());
    assert!(limits.check_bytes(4).is_err());
}

#[test]
fn rule_1_the_default_is_2_gib() {
    let limits = Limits::default();
    assert_eq!(limits.max_bytes, 2_147_483_648);
    assert!(limits.check_bytes(2_147_483_648).is_ok());
    assert!(limits.check_bytes(2_147_483_649).is_err());
}

#[test]
fn rule_2_the_entry_limit_is_checked_before_the_first_entry() {
    let limits = Limits {
        max_bytes: 1 << 20,
        max_entries: 2,
    };
    let two = manifest(&[file("a"), file("b")]);
    assert!(Manifest::decode(&two, limits).is_ok());
    let three = manifest(&[file("a"), file("b"), file("c")]);
    let err = Manifest::decode(&three, limits).unwrap_err();
    assert_eq!(rule_of(&err), (2, None));
    // The count alone decides: no entry follows it, and the manifest is not read further.
    let bare = [varint(1), varint(3)].concat();
    let err = Manifest::decode(&bare, limits).unwrap_err();
    assert_eq!(rule_of(&err), (2, None));
}

#[test]
fn rule_2_the_default_is_10_million_and_a_huge_count_reserves_nothing() {
    assert_eq!(Limits::default().max_entries, 10_000_000);
    let over = [varint(1), varint(10_000_001)].concat();
    assert_eq!(broken(&over), Some((2, None)));
    let max = [varint(1), varint(u64::MAX)].concat();
    assert_eq!(broken(&max), Some((2, None)));
    // Under the limit and over what the bytes can hold: the manifest ends early. This must not
    // reserve room for 10 million entries first.
    let short = [varint(1), varint(10_000_000), vec![0; 100]].concat();
    assert_eq!(broken(&short), Some((6, None)));
}

#[test]
fn rule_3_an_unknown_version() {
    assert_eq!(broken(&[0, 0, 0]), Some((3, None)));
    assert_eq!(broken(&[2, 0, 0]), Some((3, None)));
    assert_eq!(broken(&[0xff, 0xff, 0x03, 0, 0]), Some((3, None)));
    // Nothing after the version is read once it is wrong.
    assert_eq!(broken(&[2]), Some((3, None)));
    assert!(decode(&EMPTY).is_ok());
}

#[test]
fn rule_4_a_varint_in_a_longer_form_than_it_needs() {
    // Each field in turn gets a spelling with a zero byte too many.
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    let long = |n: u64| {
        let mut out = varint(n);
        let last = out.len() - 1;
        out[last] |= 0x80;
        out.push(0);
        out
    };
    cases.push(("version", [long(1), varint(0), varint(0)].concat()));
    cases.push(("count", [varint(1), long(0), varint(0)].concat()));
    let mut entry = file("a");
    entry.path = [long(1), b"a".to_vec()].concat();
    cases.push(("path length", manifest(&[entry])));
    for (name, set) in [
        (
            "size",
            (|r: &mut Raw, v| r.size = v) as fn(&mut Raw, Vec<u8>),
        ),
        ("mtime", |r, v| r.mtime = v),
        ("mode", |r, v| r.mode = v),
    ] {
        let mut entry = file("a");
        set(&mut entry, long(5));
        cases.push((name, manifest(&[entry])));
    }
    let mut entry = file("a");
    entry.loc = [long(0), vec![0xab; 32]].concat();
    cases.push(("loc tag", manifest(&[entry])));
    let mut entry = file("a");
    entry.loc = [varint(1), long(0), varint(0), varint(5)].concat();
    entry.link = no_link();
    cases.push(("pack_index", manifest(&[entry])));
    let mut entry = symlink("a", "b");
    entry.link = [vec![1], long(1), b"b".to_vec()].concat();
    cases.push(("link_target length", manifest(&[entry])));
    cases.push(("pack count", [varint(1), varint(0), long(0)].concat()));
    for (name, bytes) in cases {
        assert_eq!(broken(&bytes), Some((4, None)), "{name}");
    }
}

#[test]
fn rule_4_a_varint_that_does_not_fit_its_integer() {
    let eleven_bytes = [vec![0x80; 10], vec![0x01]].concat();
    let too_big_for_u64 = [vec![0xff; 9], vec![0x02]].concat();
    for bad in [eleven_bytes, too_big_for_u64] {
        let mut entry = file("a");
        entry.size = bad.clone();
        assert_eq!(broken(&manifest(&[entry])), Some((4, None)));
    }
    // The version is a u16, the mode a u32: their widest spellings are 3 and 5 bytes.
    assert_eq!(broken(&[0x81, 0x80, 0x04, 0, 0]), Some((4, None)));
    let mut entry = file("a");
    entry.mode = vec![0xff, 0xff, 0xff, 0xff, 0x1f];
    assert_eq!(broken(&manifest(&[entry])), Some((4, None)));
    // The largest that fits is accepted by the varint and then refused by rule 15 alone.
    let mut entry = file("a");
    entry.mode = vec![0xff, 0xff, 0xff, 0xff, 0x0f];
    assert_eq!(broken(&manifest(&[entry])), Some((15, None)));
    let mut entry = file("a");
    entry.size = vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(decode(&manifest(&[entry])).is_ok());
}

#[test]
fn rule_5_a_kind_or_a_tag_that_is_not_defined() {
    let mut entry = file("a");
    entry.kind = vec![3];
    assert_eq!(broken(&manifest(&[entry])), Some((5, None)));
    let mut entry = file("a");
    entry.kind = vec![0x80];
    assert_eq!(broken(&manifest(&[entry])), Some((5, None)));
    let mut entry = file("a");
    entry.loc = varint(3);
    assert_eq!(broken(&manifest(&[entry])), Some((5, None)));
    let mut entry = file("a");
    entry.link = vec![2];
    assert_eq!(broken(&manifest(&[entry])), Some((5, None)));
    let mut entry = symlink("a", "b");
    entry.link = vec![0xff];
    assert_eq!(broken(&manifest(&[entry])), Some((5, None)));
}

#[test]
fn rule_5_text_that_is_not_utf8() {
    for bad in [
        &[0xff][..],
        &[0xc3][..],
        &[0xed, 0xa0, 0x80][..],
        &[0xc0, 0x80][..],
    ] {
        let mut entry = file("a");
        entry.path = [varint(bad.len() as u64), bad.to_vec()].concat();
        assert_eq!(broken(&manifest(&[entry])), Some((5, None)), "{bad:?}");
        let mut entry = symlink("a", "b");
        entry.link = [vec![1], varint(bad.len() as u64), bad.to_vec()].concat();
        assert_eq!(broken(&manifest(&[entry])), Some((5, None)), "{bad:?}");
    }
}

#[test]
fn rule_6_every_proper_prefix_of_a_manifest_ends_early() {
    let bytes = with_packs(
        &[
            dir("d"),
            file("d/f"),
            symlink("d/l", "f"),
            Raw {
                loc: packed(0, 0, 5),
                ..file("p")
            },
        ],
        &[(7, 100)],
    );
    assert!(decode(&bytes).is_ok());
    for cut in 0..bytes.len() {
        assert_eq!(broken(&bytes[..cut]), Some((6, None)), "cut at {cut}");
    }
}

#[test]
fn rule_6_bytes_after_the_manifest() {
    let mut bytes = manifest(&[file("a")]);
    assert!(decode(&bytes).is_ok());
    bytes.push(0);
    assert_eq!(broken(&bytes), Some((6, None)));
    let mut empty = EMPTY.to_vec();
    empty.push(0);
    assert_eq!(broken(&empty), Some((6, None)));
}

#[test]
fn rule_6_a_length_that_promises_more_than_the_bytes_hold() {
    let mut entry = file("a");
    entry.path = [varint(1 << 40), b"a".to_vec()].concat();
    assert_eq!(broken(&manifest(&[entry])), Some((6, None)));
    let mut entry = symlink("a", "b");
    entry.link = [vec![1], varint(u64::MAX >> 1), b"b".to_vec()].concat();
    assert_eq!(broken(&manifest(&[entry])), Some((6, None)));
    // More packs than the bytes left could hold.
    let bytes = [varint(1), varint(0), varint(1 << 30)].concat();
    assert_eq!(broken(&bytes), Some((6, None)));
}
