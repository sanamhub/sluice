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

/// The dotless i is read as `i`, because NTFS upcases it to `I`: a volume there finds one name where
/// the text has two, so the key joins them and the receiver refuses the pair. The read comes after
/// the fold: NFKC writes U+1D6A4 as U+0131, so a read before NFKC would leave that spelling dotless
/// and let rule 9 miss the store folder it names. The NFKC after the read composes an `i` put beside
/// a mark, which is what keeps the key of a key the key.
#[test]
fn the_dotless_i_is_read_as_i() {
    assert_eq!(key("ı"), key("i"));
    assert_eq!(key("\u{131}"), "i");
    assert_eq!(key("\u{1d6a4}"), "i");
    assert_eq!(key("\u{1d6a4}"), key("\u{131}"));
    assert_eq!(key(".slu\u{1d6a4}ce-partial-x"), key(".sluice-partial-x"));
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

/// The key is NFKC, the fold, the dotless-i read, then NFKC again. The last composition joins two
/// spellings that are equal once case is folded, which a volume that resolves case reads as one.
#[test]
fn the_key_composes_after_the_fold_and_the_dotless_i_read() {
    assert_eq!(key("．sluice-partial-x"), key(".sluice-partial-x"));
    assert_eq!(key("\u{ff41}"), "a");
    // U+0390 folds to iota, diaeresis and acute; U+03AA U+0301 folds to U+03CA and an acute. The
    // two spellings are not canonically equivalent: they are equal once case is folded. The final
    // NFKC composes both to U+0390, so they have one key.
    assert_eq!(key("\u{390}"), key("\u{3aa}\u{301}"));
    assert_eq!(key("\u{390}"), "\u{390}");
    assert!(is_nfc("\u{390}"));
    // The same pair as a lowercase letter and its uppercase form, so a case mapping is what makes
    // the second spelling rather than a hand-written composition.
    assert_eq!(key("\u{3ca}\u{301}"), key("\u{3aa}\u{301}"));
}

/// A character from a block whose characters NFKC or full case folding rewrites. Latin and its
/// extended forms, Greek, Cyrillic, letterlike symbols, halfwidth and fullwidth forms, presentation
/// forms, Hangul, mathematical alphanumerics and compatibility ideographs are where a fold, a
/// composition or a case change has something to do, so a name drawn from them reaches the parts of
/// the key the examples above do not. The combining marks let a random name be canonically
/// equivalent to another without being the same text.
fn a_name_character() -> impl Strategy<Value = char> {
    prop_oneof![
        proptest::char::range('\u{20}', '\u{2ff}'),
        proptest::char::range('\u{300}', '\u{36f}'),
        proptest::char::range('\u{370}', '\u{3ff}'),
        proptest::char::range('\u{400}', '\u{4ff}'),
        proptest::char::range('\u{1e00}', '\u{1eff}'),
        proptest::char::range('\u{2100}', '\u{214f}'),
        proptest::char::range('\u{ac00}', '\u{d7a3}'),
        proptest::char::range('\u{fb00}', '\u{fb4f}'),
        proptest::char::range('\u{ff00}', '\u{ffef}'),
        proptest::char::range('\u{1d400}', '\u{1d7ff}'),
        proptest::char::range('\u{f900}', '\u{faff}'),
        proptest::char::range('\u{2f800}', '\u{2fa1f}'),
        prop::sample::select(vec![
            '\u{1d6a4}',
            '\u{301}',
            '\u{308}',
            '\u{390}',
            '\u{3aa}',
            '\u{131}',
            '\u{130}',
            '\u{200d}',
            '\u{180e}',
            '\u{a7ce}',
        ]),
    ]
}

/// A letter whose two case forms fold to texts that compose differently: one form keeps a
/// precomposed letter and the other separates the marks. `ẞ` and `İ` fold to expansions rather than
/// compositions, so a generated pair also covers the case where the last composition has nothing to
/// do.
fn a_composing_letter() -> impl Strategy<Value = char> {
    prop::sample::select(vec![
        '\u{3ca}', '\u{3cb}', '\u{390}', '\u{3b0}', '\u{3ac}', '\u{3ad}', '\u{3ae}', '\u{3af}',
        '\u{386}', '\u{388}', '\u{389}', '\u{38a}', '\u{38c}', '\u{38e}', '\u{38f}', '\u{1e9e}',
        '\u{130}',
    ])
}

/// A mark that composes with a Greek letter, so appending it to both case forms is what makes the
/// two folds differ in composition.
fn a_composing_mark() -> impl Strategy<Value = char> {
    prop::sample::select(vec!['\u{300}', '\u{301}', '\u{308}', '\u{327}', '\u{342}'])
}

proptest! {
    /// A letter and its other case are one name once a combining mark is appended. Their folds
    /// leave the composition undone differently, and only the last NFKC joins them. The NFC/NFD
    /// test cannot see this, because the first NFKC already makes those two the same text.
    #[test]
    fn a_letter_and_its_other_case_get_one_key(
        base in a_composing_letter(),
        marks in prop::collection::vec(a_composing_mark(), 1..4),
    ) {
        let tail: String = marks.into_iter().collect();
        let lower: String = base.to_lowercase().chain(tail.chars()).collect();
        let upper: String = base.to_uppercase().chain(tail.chars()).collect();
        prop_assert_eq!(key(&lower), key(&upper), "{:?} against {:?}", lower, upper);
    }

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
