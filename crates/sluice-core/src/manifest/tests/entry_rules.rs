//! Decoding rules 7 to 15 of `SPEC.md` section 8: what a list of entries has to be. Each rule gets
//! bytes a decoder reads, and where `Manifest::new` takes the same rule, the typed entries too.

use super::common::*;
use crate::manifest::{Kind, Loc, Manifest};

fn new_fails(entries: Vec<crate::manifest::Entry>, packs: Vec<crate::manifest::PackRef>) -> u8 {
    let err = Manifest::new(entries, packs).unwrap_err();
    rule_of(&err).0
}

#[test]
fn rule_7_paths_are_in_strict_byte_order() {
    assert!(decode(&manifest(&[file("a"), file("b")])).is_ok());
    assert_eq!(broken(&manifest(&[file("b"), file("a")])), Some((7, None)));
    // A repeat is a rejection and not a tie.
    assert_eq!(broken(&manifest(&[file("a"), file("a")])), Some((7, None)));
    assert_eq!(
        broken(&manifest(&[dir("a"), file("a")])),
        Some((7, None)),
        "a folder and a file of one name"
    );
    assert_eq!(new_fails(vec![typed_file("b"), typed_file("a")], vec![]), 7);
    assert_eq!(new_fails(vec![typed_file("a"), typed_file("a")], vec![]), 7);
}

#[test]
fn rule_7_the_order_is_bytes_and_not_characters_or_case() {
    // `-` is 0x2D and `/` is 0x2F, so `a-b` sorts between the folder `a` and its child.
    let ok = manifest(&[dir("a"), file("a-b"), file("a/b")]);
    assert!(decode(&ok).is_ok());
    let wrong = manifest(&[dir("a"), file("a/b"), file("a-b")]);
    assert_eq!(broken(&wrong), Some((7, None)));
    // Upper case sorts before lower case, and `é` (0xC3 0xA9) after every ASCII letter.
    assert!(decode(&manifest(&[file("B"), file("a"), file("é")])).is_ok());
    assert_eq!(broken(&manifest(&[file("a"), file("B")])), Some((7, None)));
    assert_eq!(broken(&manifest(&[file("é"), file("z")])), Some((7, None)));
}

#[test]
fn rule_8_the_parent_is_a_folder_entry_before_the_entry() {
    assert!(decode(&manifest(&[dir("a"), dir("a/b"), file("a/b/c")])).is_ok());
    // No entry for the parent.
    assert_eq!(broken(&manifest(&[file("a/b")])), Some((8, None)));
    // A grandparent without an entry for the parent between.
    assert_eq!(
        broken(&manifest(&[dir("a"), file("a/b/c")])),
        Some((8, None))
    );
    // A parent that is a file or a symlink.
    assert_eq!(
        broken(&manifest(&[file("a"), file("a/b")])),
        Some((8, None))
    );
    assert_eq!(
        broken(&manifest(&[symlink("a", "x"), file("a/b")])),
        Some((8, None))
    );
    // The folder that was sent is not an entry, so a top-level path has no parent to look for.
    assert!(decode(&manifest(&[file("top")])).is_ok());
    assert_eq!(new_fails(vec![typed_file("a/b")], vec![]), 8);
    assert_eq!(
        new_fails(vec![typed_file("a"), typed_file("a/b")], vec![]),
        8
    );
}

#[test]
fn rule_9_a_path_that_breaks_a_path_rule_names_the_path_rule() {
    let long = "x".repeat(256);
    let cases = [
        ("", 1),
        ("e\u{301}", 2),
        ("/a", 3),
        ("a/", 3),
        ("a//b", 4),
        ("a/./b", 4),
        ("a/../b", 4),
        (long.as_str(), 5),
        ("a\\b", 6),
        ("a:b", 6),
        ("a?", 6),
        ("a ", 7),
        ("a.", 7),
        ("con", 8),
        ("Nul.txt", 8),
    ];
    for (path, sub) in cases {
        assert_eq!(
            broken(&manifest(&[file(path)])),
            Some((9, Some(sub))),
            "{path:?}"
        );
    }
    assert!(decode(&manifest(&[file("CONSOLE"), file("a.con")])).is_ok());
}

#[test]
fn rule_10_link_target_is_present_exactly_on_a_symlink() {
    let mut bare = symlink("a", "b");
    bare.link = no_link();
    assert_eq!(broken(&manifest(&[bare])), Some((10, None)));
    let mut extra = file("a");
    extra.link = link("b");
    assert_eq!(broken(&manifest(&[extra])), Some((10, None)));
    let mut extra = dir("a");
    extra.link = link("b");
    assert_eq!(broken(&manifest(&[extra])), Some((10, None)));
    assert!(decode(&manifest(&[symlink("a", "b")])).is_ok());

    let mut no_target = typed_link("a", "b");
    no_target.link_target = None;
    assert_eq!(new_fails(vec![no_target], vec![]), 10);
    let mut stray = typed_file("a");
    stray.link_target = typed_link("x", "y").link_target;
    assert_eq!(new_fails(vec![stray], vec![]), 10);
}

#[test]
fn rule_11_a_link_target_that_breaks_a_link_target_rule_names_it() {
    let cases = [
        ("", 1),
        ("/etc/passwd", 2),
        ("a/", 2),
        ("a//b", 3),
        ("./a", 3),
        ("C:\\x", 4),
        ("\\\\server\\share", 4),
        ("a/con", 4),
    ];
    for (target, sub) in cases {
        assert_eq!(
            broken(&manifest(&[symlink("l", target)])),
            Some((11, Some(sub))),
            "{target:?}"
        );
    }
}

#[test]
fn rule_11_a_target_that_steps_above_the_output_folder() {
    assert_eq!(
        broken(&manifest(&[symlink("link", "../x")])),
        Some((11, Some(5)))
    );
    assert!(decode(&manifest(&[dir("a"), symlink("a/link", "../b")])).is_ok());
    assert_eq!(
        broken(&manifest(&[dir("a"), symlink("a/link", "../../b")])),
        Some((11, Some(5)))
    );
    assert_eq!(new_fails(vec![typed_link("link", "../x")], vec![]), 11);
    assert!(Manifest::new(vec![typed_dir("a"), typed_link("a/link", "../x")], vec![]).is_ok());
}

#[test]
fn rule_12_two_paths_that_are_the_same_after_case_folding() {
    let collide = |a: &str, b: &str| {
        let (first, second) = if a < b { (a, b) } else { (b, a) };
        let bytes = manifest(&[file(first), file(second)]);
        assert_eq!(broken(&bytes), Some((12, None)), "{a:?} and {b:?}");
        let typed = vec![typed_file(first), typed_file(second)];
        assert_eq!(new_fails(typed, vec![]), 12, "{a:?} and {b:?}");
    };
    collide("README.md", "Readme.md");
    collide("README.md", "readme.md");
    // Full folding: sharp s is `ss`, and the capital one is too.
    collide("straße", "STRASSE");
    collide("straße", "strasse");
    collide("ẞ", "ss");
    // The long s folds to `s`. The Kelvin sign would fold to `k`, but it is not in NFC, so rule 9
    // refuses it before rule 12 can.
    collide("ſ", "s");
    collide("ς", "Σ");
    collide("σ", "ς");
    // The ligature `ﬃ` folds to `ffi`.
    collide("ﬃ", "FFI");
}

#[test]
fn rule_12_through_a_folder() {
    // `A/x` and `a/y` are different paths, and the folders `A` and `a` are the same one.
    let bytes = manifest(&[dir("A"), file("A/x"), dir("a"), file("a/y")]);
    assert_eq!(broken(&bytes), Some((12, None)));
    let typed = vec![
        typed_dir("A"),
        typed_file("A/x"),
        typed_dir("a"),
        typed_file("a/y"),
    ];
    assert_eq!(new_fails(typed, vec![]), 12);
    // A file and a folder of one name collide as well as two of a kind.
    let bytes = manifest(&[file("A"), dir("a")]);
    assert_eq!(broken(&bytes), Some((12, None)));
}

#[test]
fn rule_12_does_not_reject_names_that_differ() {
    // `i` and `ı` (dotless) are two letters: only a Turkic folding joins them.
    assert!(decode(&manifest(&[file("i"), file("ı")])).is_ok());
    assert!(decode(&manifest(&[dir("a"), file("a/x"), file("a/y")])).is_ok());
    assert!(decode(&manifest(&[file("ss"), file("sss")])).is_ok());
    // Same letters at different depths are different paths.
    assert!(decode(&manifest(&[dir("a"), file("a/a"), file("b")])).is_ok());
}

#[test]
fn rule_13_kind_loc_and_size_agree() {
    let with = |entry: Raw| broken(&manifest(&[entry]));
    // A file has a blob or a packed range. It never has no loc.
    let mut file_without = file("a");
    file_without.loc = no_loc();
    assert_eq!(with(file_without), Some((13, None)));
    // A folder and a symlink have no loc and a size of 0.
    let mut folder_blob = dir("a");
    folder_blob.loc = blob(1);
    assert_eq!(with(folder_blob), Some((13, None)));
    let mut folder_packed = dir("a");
    folder_packed.loc = packed(0, 0, 0);
    assert_eq!(with(folder_packed), Some((13, None)));
    let mut sized_folder = dir("a");
    sized_folder.size = varint(1);
    assert_eq!(with(sized_folder), Some((13, None)));
    let mut sized_link = symlink("a", "b");
    sized_link.size = varint(1);
    assert_eq!(with(sized_link), Some((13, None)));
    let mut link_blob = symlink("a", "b");
    link_blob.loc = blob(1);
    assert_eq!(with(link_blob), Some((13, None)));
    // A zero-size file is a file.
    let mut empty = file("a");
    empty.size = varint(0);
    assert_eq!(with(empty), None);

    let bad = typed("a", Kind::Dir, Loc::Blob { root: [0; 32] }, 0);
    assert_eq!(new_fails(vec![bad], vec![]), 13);
    let bad = typed("a", Kind::Symlink, Loc::None, 4);
    let bad = crate::manifest::Entry {
        link_target: typed_link("x", "y").link_target,
        ..bad
    };
    assert_eq!(new_fails(vec![bad], vec![]), 13);
    assert_eq!(
        new_fails(vec![typed("a", Kind::File, Loc::None, 0)], vec![]),
        13
    );
}

fn in_pack(offset: u64, len: u64, size: u64, pack_index: u32) -> Raw {
    Raw {
        size: varint(size),
        loc: packed(pack_index, offset, len),
        ..file("a")
    }
}

#[test]
fn rule_14_a_packed_entry_fits_its_pack() {
    let packs = [(9u8, 100u64)];
    let ok = |entry: Raw| decode(&with_packs(&[entry], &packs)).is_ok();
    let bad = |entry: Raw| broken(&with_packs(&[entry], &packs));
    assert!(ok(in_pack(0, 100, 100, 0)));
    assert!(ok(in_pack(95, 5, 5, 0)));
    // A file of no bytes at the very end of the pack.
    assert!(ok(in_pack(100, 0, 0, 0)));
    assert_eq!(bad(in_pack(0, 101, 101, 0)), Some((14, None)));
    assert_eq!(bad(in_pack(96, 5, 5, 0)), Some((14, None)));
    assert_eq!(bad(in_pack(101, 0, 0, 0)), Some((14, None)));
    // `len` is the size. The two cannot differ.
    assert_eq!(bad(in_pack(0, 5, 6, 0)), Some((14, None)));
    assert_eq!(bad(in_pack(0, 6, 5, 0)), Some((14, None)));
    // The index is below the number of packs.
    assert_eq!(bad(in_pack(0, 5, 5, 1)), Some((14, None)));
    assert_eq!(bad(in_pack(0, 5, 5, u32::MAX)), Some((14, None)));
    // `offset + len` does not wrap.
    assert_eq!(bad(in_pack(u64::MAX, 2, 2, 0)), Some((14, None)));
    // No packs at all.
    let none = manifest(&[in_pack(0, 5, 5, 0)]);
    assert_eq!(broken(&none), Some((14, None)));
}

#[test]
fn rule_14_for_typed_entries_and_with_several_packs() {
    use crate::manifest::PackRef;
    let packs = vec![
        PackRef {
            root: [1; 32],
            size: 10,
        },
        PackRef {
            root: [2; 32],
            size: 1000,
        },
    ];
    let at = |pack_index, offset, len| {
        typed(
            "a",
            Kind::File,
            Loc::Packed {
                pack_index,
                offset,
                len,
            },
            len,
        )
    };
    assert!(Manifest::new(vec![at(1, 990, 10)], packs.clone()).is_ok());
    assert_eq!(new_fails(vec![at(0, 990, 10)], packs.clone()), 14);
    assert_eq!(new_fails(vec![at(2, 0, 1)], packs.clone()), 14);
    let wrong_size = typed(
        "a",
        Kind::File,
        Loc::Packed {
            pack_index: 0,
            offset: 0,
            len: 5,
        },
        6,
    );
    assert_eq!(new_fails(vec![wrong_size], packs), 14);
}

#[test]
fn rule_15_mode_holds_permission_bits_only() {
    let with = |mode: u64| {
        let mut entry = file("a");
        entry.mode = varint(mode);
        broken(&manifest(&[entry]))
    };
    assert_eq!(with(0), None);
    assert_eq!(with(0o644), None);
    // Setuid, setgid and sticky are bits of the mode a manifest may carry. A receiver does not
    // apply the first two, and that is a rule for the writer and not for the decoder.
    assert_eq!(with(0o7777), None);
    assert_eq!(with(0o10000), Some((15, None)));
    assert_eq!(with(0o100644), Some((15, None)));
    assert_eq!(with(u64::from(u32::MAX)), Some((15, None)));
    let mut over = typed_file("a");
    over.mode = 0o10000;
    assert_eq!(new_fails(vec![over], vec![]), 15);
    let mut top = typed_file("a");
    top.mode = 0o7777;
    assert!(Manifest::new(vec![top], vec![]).is_ok());
}

#[test]
fn a_symlink_may_have_any_mode_up_to_the_limit() {
    // The sender writes 0. A receiver has no rule that says it must read 0.
    let mut entry = symlink("a", "b");
    entry.mode = varint(0o777);
    assert!(decode(&manifest(&[entry])).is_ok());
}

#[test]
fn the_empty_manifest_and_a_rich_one_decode_and_encode_to_the_same_bytes() {
    let empty = decode(&[1, 0, 0]).expect("empty");
    assert!(empty.entries().is_empty() && empty.packs().is_empty());
    assert_eq!(empty.encode(), [1, 0, 0]);

    let bytes = with_packs(
        &[
            dir("d"),
            file("d/f"),
            symlink("d/l", "f"),
            Raw {
                loc: packed(0, 10, 5),
                ..file("p")
            },
        ],
        &[(7, 100)],
    );
    let manifest = decode(&bytes).expect("rich");
    assert_eq!(manifest.encode(), bytes);
    assert_eq!(manifest.root(), *blake3::hash(&bytes).as_bytes());
    let kinds: Vec<Kind> = manifest.entries().iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [Kind::Dir, Kind::File, Kind::Symlink, Kind::File]);
}
