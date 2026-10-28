//! The one key a name is compared by: `SPEC.md` section 8 path rule 9 defines it, and the collision
//! rule and the link check use it.

use proptest::prelude::*;
use unicode_normalization::{UNICODE_VERSION, UnicodeNormalization, is_nfc};

use super::common::*;
use crate::manifest::path::fold_key as key;

#[test]
fn full_folding_joins_a_string_with_its_expansion() {
    assert_eq!(key("Readme.md"), key("README.md"));
    assert_eq!(key("straße"), "strasse");
    assert_eq!(key("STRASSE"), "strasse");
    // U+1E9E and U+00DF both fold to `ss`.
    assert_eq!(key("ẞ"), "ss");
    // A ligature folds to the letters it stands for, which are three.
    assert_eq!(key("ﬃ"), "ffi");
    assert_ne!(key("a"), key("b"));
}

/// The dotless i is read as `i` before the fold, because NTFS upcases it to `I`: a volume there
/// finds one name where the text has two, so the key joins them and the receiver refuses the pair.
#[test]
fn the_dotless_i_is_read_as_i() {
    assert_eq!(key("ı"), key("i"));
    assert_eq!(key("\u{131}"), "i");
}

/// The code points a name may not show are read away, because a file system may compare a name as
/// though they were not there. The whole set is in the generated table's own test; these are one
/// from each corner of it, including U+180E and the tag range.
#[test]
fn a_default_ignorable_code_point_is_read_away() {
    for name in [
        "a\u{200d}b",
        "a\u{feff}b",
        "a\u{ad}b",
        "a\u{fff0}b",
        "a\u{180e}b",
        "a\u{e01f0}b",
    ] {
        assert_eq!(key(name), key("ab"), "{name:?}");
    }
    // Not every code point that looks invisible is ignorable: U+00A0 is a space, and the key keeps
    // it, so a name that carries one is not the name without it.
    assert_ne!(key("a\u{a0}b"), key("ab"));
}

#[test]
fn the_tables_are_those_of_unicode_18() {
    // U+A7CE and U+A7CF were added in Unicode 18.0.0 and U+A7CB in 16.0.0. Tables from before
    // 18 would leave the first one alone, and a manifest of two names that differ in it would be
    // accepted here and rejected by a receiver on the right tables.
    assert_eq!(key("\u{a7ce}"), "\u{a7cf}");
    assert_eq!(key("\u{a7cb}"), "\u{264}");
}

#[test]
fn the_nfc_tables_are_those_of_unicode_18() {
    assert_eq!(
        UNICODE_VERSION,
        (18, 0, 0),
        "unicode-normalization moved to other tables. They decide which paths are NFC (path \
         rule 2), so a change of version is a change of SPEC.md section 8, with new vectors"
    );
}

/// The key is NFKC, then the fold, then NFKC again. The last composition joins names that are
/// canonically equivalent, which a volume that compares them reads as one name.
#[test]
fn the_key_is_nfkc_then_the_fold_then_nfkc_again() {
    assert_eq!(key("．sluice-partial-x"), key(".sluice-partial-x"));
    assert_eq!(key("\u{ff41}"), "a");
    // U+0390 folds to iota, diaeresis and acute; U+03AA U+0301 folds to U+03CA and an acute. The
    // final NFKC composes both to U+0390, so the two names have one key.
    assert_eq!(key("\u{390}"), key("\u{3aa}\u{301}"));
    assert_eq!(key("\u{390}"), "\u{390}");
    assert!(is_nfc("\u{390}"));
}

/// Characters whose folding, composition or case is subtle, so a generated name reaches the parts
/// of the key the examples above do not. Latin, Greek, Cyrillic and Hangul are the scripts a name
/// is likely to be in, and the combining marks let a random name be canonically equivalent to
/// another without being the same text.
fn a_name_character() -> impl Strategy<Value = char> {
    prop::sample::select(vec![
        'a', 'A', 'z', 'Z', ' ', '0', 'ß', 'ẞ', 'é', 'E', 'İ', 'ı', '\u{ff41}', '\u{ff21}',
        '\u{301}', '\u{300}', '\u{308}', '\u{327}', '\u{390}', '\u{3aa}', '\u{3ca}', '\u{3b9}',
        '\u{3b1}', '\u{3a9}', '\u{3c2}', '\u{3c3}', '\u{410}', '\u{415}', '\u{435}', '\u{401}',
        '\u{ac00}', '\u{1100}', '\u{1161}', '\u{1112}', '\u{1175}', '\u{a7ce}', '\u{200d}',
    ])
}

proptest! {
    /// A name and its NFC and NFD forms are one name: the key is taken after a composition, so the
    /// order the sender wrote the marks in cannot decide whether two entries collide.
    #[test]
    fn canonically_equivalent_names_get_one_key(chars in prop::collection::vec(a_name_character(), 1..8)) {
        let text: String = chars.into_iter().collect();
        let composed: String = text.nfc().collect();
        let decomposed: String = text.nfd().collect();
        prop_assert_eq!(key(&composed), key(&decomposed), "{:?}", text);
    }

    /// The key of a key is the key. The decoder compares keys and the collision set holds their
    /// hashes, so a key that changed when it was keyed again would be two answers to one question.
    #[test]
    fn the_key_does_not_change_when_it_is_keyed_again(chars in prop::collection::vec(a_name_character(), 1..8)) {
        let text: String = chars.into_iter().collect();
        prop_assert_eq!(key(&key(&text)), key(&text), "{:?}", text);
    }
}

#[test]
fn a_key_is_made_of_whole_paths_so_slashes_decide_nothing() {
    assert_ne!(key("a/b"), key("ab"));
    assert_eq!(key("A/B"), key("a/b"));
    assert_ne!(key("a/x"), key("a/y"));
}

#[test]
fn two_folders_that_collide_are_found_whatever_is_inside() {
    let bytes = manifest(&[dir("A"), dir("a")]);
    assert_eq!(broken(&bytes), Some((12, None)));
}
