//! Names a receiver would reject: every one is listed with its rule, nothing is hashed, and the
//! folder is refused. Names that Windows cannot create are in the `cfg(unix)` tests, which run in
//! WSL.

use super::common::*;
use crate::Error;
use crate::manifest::build::build;
use crate::manifest::{Build, Kind, Limits, Manifest, Reason};

/// The decomposed spelling of `é`: `e` and a combining acute accent.
const E_ACUTE_NFD: &str = "e\u{301}";

#[test]
fn a_name_in_nfd_is_sent_in_nfc() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(
        dir.path(),
        &format!("caf{E_ACUTE_NFD}/men{E_ACUTE_NFD}.txt"),
        b"x",
    );
    let built = ready(dir.path());
    assert_eq!(
        listing(&built),
        [("café", Kind::Dir), ("café/mené.txt", Kind::File)]
    );
}

#[test]
fn two_names_that_become_one_path_after_nfc_refuse_the_build() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "caf\u{e9}", b"composed");
    put(dir.path(), &format!("caf{E_ACUTE_NFD}"), b"decomposed");
    let refusal = refused(dir.path());
    assert_eq!(refusal.problems.len(), 2);
    assert!(
        refusal
            .problems
            .iter()
            .all(|f| f.reason == Reason::SameAfterNfc)
    );
    let shown: Vec<&str> = refusal.problems.iter().map(|f| f.path.as_str()).collect();
    // Each is listed as the sender sees it, which is how it is spelled on disk.
    assert!(shown.contains(&"caf\u{e9}") && shown.contains(&format!("caf{E_ACUTE_NFD}").as_str()));
}

// ext4 and APFS hold a name of 255 bytes at most, so a Unix sender can never meet rule 5. NTFS
// counts UTF-16 units, so 128 characters of two bytes fit there and are 256 bytes in UTF-8.
#[cfg(windows)]
#[test]
fn a_component_of_256_bytes_is_rule_5() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let long = "é".repeat(128);
    put(dir.path(), &long, b"x");
    put(dir.path(), &"é".repeat(127), b"fine");
    let refusal = refused(dir.path());
    assert_eq!(refusal.problems, [finding(&long, Reason::PathRule(5))]);
}

#[test]
fn bidirectional_and_c1_controls_are_rule_6() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let bidi = "report\u{202e}fdp.exe";
    let c1 = "line\u{85}break";
    put(dir.path(), bidi, b"x");
    put(dir.path(), c1, b"x");
    // Zero-width characters are accepted.
    put(dir.path(), "zero\u{200b}width", b"x");
    let refusal = refused(dir.path());
    assert_eq!(
        refusal.problems,
        [
            finding(c1, Reason::PathRule(6)),
            finding(bidi, Reason::PathRule(6))
        ]
    );
}

#[test]
fn every_bad_name_is_listed_in_one_run_sorted_by_path() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "z\u{202e}", b"x");
    put(dir.path(), "ok/fine.txt", b"x");
    put(dir.path(), "a/b\u{202e}c", b"x");
    put(dir.path(), "a/two\u{85}", b"x");
    put(dir.path(), "dir\u{202e}/x", b"x");
    let refusal = refused(dir.path());
    let listed: Vec<(&str, Reason)> = refusal
        .problems
        .iter()
        .map(|f| (f.path.as_str(), f.reason))
        .collect();
    assert_eq!(
        listed,
        [
            ("a/b\u{202e}c", Reason::PathRule(6)),
            ("a/two\u{85}", Reason::PathRule(6)),
            ("dir\u{202e}", Reason::PathRule(6)),
            ("z\u{202e}", Reason::PathRule(6)),
        ]
    );
}

#[test]
fn a_folder_with_a_bad_name_is_listed_once_and_what_is_wrong_below_it_still_is() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let bad_folder = "dir\u{202e}";
    put(dir.path(), &format!("{bad_folder}/fine.txt"), b"x");
    put(dir.path(), &format!("{bad_folder}/inner\u{85}"), b"x");
    let refusal = refused(dir.path());
    let listed: Vec<&str> = refusal.problems.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        listed,
        [bad_folder, format!("{bad_folder}/inner\u{85}").as_str()]
    );
}

#[test]
fn nothing_is_hashed_when_a_name_is_refused() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "big.bin", &[0; 1024]);
    put(dir.path(), "bad\u{202e}", b"x");
    let result = build(dir.path(), Limits::default(), |_| {
        panic!("a file was read although the build was going to be refused")
    });
    assert!(matches!(result, Ok(Build::Refused(_))));
}

#[test]
fn the_refusal_error_holds_counts_and_no_name() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "private-name\u{202e}.txt", b"x");
    put(dir.path(), "other-private\u{85}", b"x");
    let refusal = refused(dir.path());
    let err = refusal.error();
    assert!(matches!(err, Error::Policy(_)));
    let message = err.to_string();
    assert!(message.contains("2 names"), "{message}");
    assert!(!message.contains("private"), "{message}");
}

#[test]
fn a_clean_folder_is_not_refused() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a/b.txt", b"x");
    assert!(matches!(
        Manifest::build(dir.path()).expect("the build runs"),
        Build::Ready(_)
    ));
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    use super::super::common::*;
    use crate::manifest::Reason;

    fn names(refusal: &crate::manifest::Refusal) -> Vec<(&str, Reason)> {
        refusal
            .problems
            .iter()
            .map(|f| (f.path.as_str(), f.reason))
            .collect()
    }

    #[test]
    fn a_name_that_is_not_utf8_refuses_the_build_and_is_shown_lossily() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let name = OsStr::from_bytes(b"bad-\xff-name");
        std::fs::write(dir.path().join(name), b"x").expect("write");
        let refusal = refused(dir.path());
        assert_eq!(names(&refusal), [("bad-\u{fffd}-name", Reason::NotUtf8)]);
    }

    #[test]
    fn path_rules_6_7_and_8_each_refuse_a_name() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        for name in [
            "a:b",
            "a\\b",
            "a?b",
            "trailing.",
            "trailing ",
            "con",
            "Nul.txt",
            "LPT1",
        ] {
            put(dir.path(), name, b"x");
        }
        put(dir.path(), "ok.txt", b"x");
        let refusal = refused(dir.path());
        assert_eq!(
            names(&refusal),
            [
                ("LPT1", Reason::PathRule(8)),
                ("Nul.txt", Reason::PathRule(8)),
                ("a:b", Reason::PathRule(6)),
                ("a?b", Reason::PathRule(6)),
                ("a\\b", Reason::PathRule(6)),
                ("con", Reason::PathRule(8)),
                ("trailing ", Reason::PathRule(7)),
                ("trailing.", Reason::PathRule(7)),
            ]
        );
    }

    #[test]
    fn two_names_that_differ_only_in_case_refuse_the_build() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "Readme.md", b"a");
        put(dir.path(), "README.md", b"b");
        put(dir.path(), "other.txt", b"c");
        let refusal = refused(dir.path());
        assert_eq!(
            names(&refusal),
            [
                ("README.md", Reason::CaseCollision),
                ("Readme.md", Reason::CaseCollision)
            ]
        );
    }

    #[test]
    fn sharp_s_against_ss_collides_through_full_folding() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "straße", b"a");
        put(dir.path(), "STRASSE", b"b");
        let refusal = refused(dir.path());
        assert_eq!(refusal.problems.len(), 2);
        assert!(
            refusal
                .problems
                .iter()
                .all(|f| f.reason == Reason::CaseCollision)
        );
    }

    #[test]
    fn two_folders_that_differ_in_case_collide_whatever_is_inside() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "A/x", b"1");
        put(dir.path(), "a/y", b"2");
        let refusal = refused(dir.path());
        assert_eq!(
            names(&refusal),
            [("A", Reason::CaseCollision), ("a", Reason::CaseCollision)]
        );
    }

    #[test]
    fn a_collision_of_two_and_a_twin_are_told_apart() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "caf\u{e9}", b"1");
        put(dir.path(), "cafe\u{301}", b"2");
        put(dir.path(), "CAF\u{c9}", b"3");
        let refusal = refused(dir.path());
        let listed = names(&refusal);
        assert_eq!(listed.len(), 3);
        // Two names are one path after NFC, and the third differs from them only in case.
        let twins = listed
            .iter()
            .filter(|(_, r)| *r == Reason::SameAfterNfc)
            .count();
        let cases = listed
            .iter()
            .filter(|(_, r)| *r == Reason::CaseCollision)
            .count();
        assert_eq!((twins, cases), (2, 1));
    }
}

// Linux and macOS refuse to create a path of more than 4096 bytes in one call, so only Windows,
// which takes up to 32,767 characters, can have a tree that crosses the limit of path rule 1.
#[cfg(windows)]
mod windows {
    use super::super::common::*;
    use crate::manifest::Reason;

    #[test]
    fn a_path_longer_than_4096_bytes_is_rule_1_for_the_entry_that_crosses_it() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        // 17 folders of 250 bytes, then a file: the path is over 4096 bytes at the 17th level.
        let level = "d".repeat(250);
        let mut rel = String::new();
        for _ in 0..17 {
            rel.push_str(&level);
            rel.push('/');
        }
        mkdir(dir.path(), &rel);
        put(dir.path(), &format!("{rel}file"), b"x");
        let refusal = refused(dir.path());
        let rules: Vec<Reason> = refusal.problems.iter().map(|f| f.reason).collect();
        assert!(!rules.is_empty());
        assert!(rules.iter().all(|r| *r == Reason::PathRule(1)), "{rules:?}");
    }
}
