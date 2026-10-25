//! The build's collision check without a file system. A case-insensitive or
//! normalization-insensitive volume cannot hold two spellings of one name, and the default macOS
//! volume is both, so these pairs are built here and the walk is not involved.

use std::path::PathBuf;

use super::super::collide::find;
use super::super::walk::{Item, What};
use crate::manifest::Reason;
use crate::manifest::path::ManifestPath;

fn item(path: &str, shown: &str) -> Item {
    Item {
        path: ManifestPath::parse(path).expect("a valid path"),
        shown: shown.to_string(),
        what: What::File {
            disk: PathBuf::from(path),
        },
    }
}

#[test]
fn two_paths_that_differ_only_in_case_are_both_case_collisions() {
    let found = find(&[
        item("README.md", "README.md"),
        item("Readme.md", "Readme.md"),
    ]);
    assert_eq!(found.len(), 2);
    assert!(
        found.iter().all(|f| f.reason == Reason::CaseCollision),
        "{found:?}"
    );
}

/// Full case folding, not an ASCII lower case: the sharp s folds to `ss`.
#[test]
fn full_case_folding_collides_across_scripts() {
    let found = find(&[
        item("stra\u{df}e", "stra\u{df}e"),
        item("STRASSE", "STRASSE"),
    ]);
    assert_eq!(found.len(), 2);
    assert!(
        found.iter().all(|f| f.reason == Reason::CaseCollision),
        "{found:?}"
    );
}

/// A folder collides even when what is inside it has different names: the folder is one entry to
/// the receiver either way.
#[test]
fn two_folders_whose_names_differ_in_case_collide() {
    let found = find(&[
        item("A", "A"),
        item("a", "a"),
        item("A/x", "A/x"),
        item("a/y", "a/y"),
    ]);
    let folders: Vec<_> = found
        .iter()
        .filter(|f| f.path == "A" || f.path == "a")
        .collect();
    assert_eq!(folders.len(), 2, "{found:?}");
    assert!(
        folders.iter().all(|f| f.reason == Reason::CaseCollision),
        "{found:?}"
    );
}

/// Two spellings of one NFC path are told apart from a name that differs only in case.
#[test]
fn a_twin_after_nfc_is_told_apart_from_a_case_collision() {
    let found = find(&[
        item("caf\u{e9}", "caf\u{e9}"),
        item("caf\u{e9}", "cafe\u{301}"),
        item("CAF\u{c9}", "CAF\u{c9}"),
    ]);
    let twins = found
        .iter()
        .filter(|f| f.reason == Reason::SameAfterNfc)
        .count();
    let cases = found
        .iter()
        .filter(|f| f.reason == Reason::CaseCollision)
        .count();
    assert_eq!((twins, cases), (2, 1), "{found:?}");
}
