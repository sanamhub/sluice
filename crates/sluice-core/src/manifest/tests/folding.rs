//! The collision key of `SPEC.md` section 8 rule 12: full Unicode case folding, then NFC.

use unicode_normalization::is_nfc;

use super::common::*;
use crate::manifest::rules::collision_key;

#[test]
fn full_folding_joins_a_string_with_its_expansion() {
    assert_eq!(collision_key("Readme.md"), collision_key("README.md"));
    assert_eq!(collision_key("straße"), "strasse");
    assert_eq!(collision_key("STRASSE"), "strasse");
    // U+1E9E and U+00DF both fold to `ss`.
    assert_eq!(collision_key("ẞ"), "ss");
    // A ligature folds to the letters it stands for, which are three.
    assert_eq!(collision_key("ﬃ"), "ffi");
    assert_ne!(collision_key("a"), collision_key("b"));
    // The dotless i is not the lower case of `I` outside Turkic locales, and Sluice has none.
    assert_ne!(collision_key("ı"), collision_key("i"));
}

#[test]
fn the_tables_are_those_of_unicode_18() {
    // U+A7CE and U+A7CF were added in Unicode 18.0.0 and U+A7CB in 16.0.0. Tables from before
    // 18 would leave the first one alone, and a manifest of two names that differ in it would be
    // accepted here and rejected by a receiver on the right tables.
    assert_eq!(collision_key("\u{a7ce}"), "\u{a7cf}");
    assert_eq!(collision_key("\u{a7cb}"), "\u{264}");
    assert_eq!(unicode_normalization::UNICODE_VERSION, (18, 0, 0));
}

#[test]
fn nfc_follows_the_fold_because_a_fold_can_leave_nfc() {
    // U+0390 folds to three code points, which are not NFC. `Ϊ` and a combining acute fold to two
    // code points that NFC composes into U+0390. The names are one name only if NFC comes last.
    let precomposed = "\u{390}";
    let separate = "\u{3aa}\u{301}";
    assert!(is_nfc(precomposed) && is_nfc(separate));
    assert_eq!(collision_key(precomposed), collision_key(separate));
    assert!(is_nfc(&collision_key(precomposed)));
}

#[test]
fn a_key_is_made_of_whole_paths_so_slashes_decide_nothing() {
    assert_ne!(collision_key("a/b"), collision_key("ab"));
    assert_eq!(collision_key("A/B"), collision_key("a/b"));
    assert_ne!(collision_key("a/x"), collision_key("a/y"));
}

#[test]
fn two_folders_that_collide_are_found_whatever_is_inside() {
    let bytes = manifest(&[dir("A"), dir("a")]);
    assert_eq!(broken(&bytes), Some((12, None)));
}
