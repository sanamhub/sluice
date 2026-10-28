//! The generated `Default_Ignorable_Code_Point` table and what the key does with it.

use super::fold_key;
use super::ignorable::{RANGES, is_ignorable};

/// The table is the property of the version `SPEC.md` section 8 names, and it is a sorted list of
/// ranges with no neighbours that could have been merged. A hand list lost U+180E and the whole of
/// U+E01F0 to U+E0FFF once, which is why the count and the edges are checked here.
#[test]
fn the_table_is_the_property_of_unicode_18() {
    let total: u32 = RANGES.iter().map(|(first, last)| last - first + 1).sum();
    assert_eq!(total, 4174, "the table is not the set Unicode 18 names");
    assert!(
        RANGES
            .windows(2)
            .all(|pair| pair[0].1.saturating_add(1) < pair[1].0),
        "the ranges are not sorted with adjacent ranges merged"
    );
    for code in [0x180E, 0xE0000, 0xE0FFF] {
        let c = char::from_u32(code).expect("a code point");
        assert!(is_ignorable(c), "U+{code:04X}");
    }
    for code in [0x180A, 0x1810, 0xE1000, 0x10FFFF] {
        let c = char::from_u32(code).expect("a code point");
        assert!(!is_ignorable(c), "U+{code:04X}");
    }
}

/// Every code point in the table is read away from a name. The code points on either side of it
/// are not, so a name that carries one is still another name.
#[test]
fn every_code_point_in_the_table_is_read_away() {
    let mut seen = 0u32;
    for (first, last) in RANGES {
        for code in first..=last {
            seen += 1;
            let name = char::from_u32(code).expect("a code point");
            assert_eq!(
                fold_key(&format!("a{name}b")),
                fold_key("ab"),
                "U+{code:04X}"
            );
        }
    }
    assert_eq!(seen, 4174);
}

#[test]
fn a_code_point_next_to_the_table_is_kept() {
    // U+180F is a variation selector and is ignored; U+1810 is a letter and is not.
    assert_eq!(fold_key("a\u{180f}b"), fold_key("ab"));
    assert_ne!(fold_key("a\u{1810}b"), fold_key("ab"));
    // The last range ends at U+E0FFF; the tag characters above it are not in the property.
    assert_ne!(fold_key("a\u{e1000}b"), fold_key("ab"));
}
