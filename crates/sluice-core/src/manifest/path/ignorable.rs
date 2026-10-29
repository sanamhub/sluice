//! The `Default_Ignorable_Code_Point` property of Unicode 18.0.0, written by
//! `cargo xtask unicode-ignorable` from the Unicode Character Database.
//!
//! # DerivedCoreProperties-18.0.0.txt
//! # Date: 2026-08-07, 16:19:42 GMT
//!
//! 4174 code points in 17 ranges, adjacent code points merged.

/// The ranges, sorted, with the first and the last code point of each.
pub(super) const RANGES: [(u32, u32); 17] = [
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0x3164, 0x3164),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFF8),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0FFF),
];

/// Whether `c` carries the property: a name may keep one and a file system may compare it
/// as though it were not there.
pub(super) fn is_ignorable(c: char) -> bool {
    let code = u32::from(c);
    RANGES
        .binary_search_by(|&(first, last)| {
            if last < code {
                std::cmp::Ordering::Less
            } else if first > code {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
