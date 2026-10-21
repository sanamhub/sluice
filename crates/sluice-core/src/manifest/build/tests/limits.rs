//! A sender refuses a folder whose manifest a receiver would reject for its size, as rules 1 and
//! 2 of `SPEC.md` section 8 say, so it never sends one that cannot arrive.

use super::common::*;
use crate::Error;
use crate::manifest::build::build;
use crate::manifest::{Build, Limits, Manifest};

fn entries(max_entries: u64) -> Limits {
    Limits {
        max_entries,
        ..Limits::default()
    }
}

#[test]
fn a_folder_over_the_entry_limit_fails_before_any_file_is_hashed() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "secret-a.txt", b"x");
    put(dir.path(), "secret-b.txt", b"y");
    put(dir.path(), "secret-c.txt", b"z");
    let result = build(dir.path(), entries(2), |_| {
        panic!("a file was read although the build was over the entry limit")
    });
    let Err(Error::Policy(message)) = result else {
        panic!("expected a policy error");
    };
    assert!(message.contains("3 entries"), "{message}");
    assert!(message.contains("at most 2"), "{message}");
    assert!(!message.contains("secret"), "{message}");
}

#[test]
fn the_entry_limit_counts_folders_and_a_folder_at_it_is_built() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "docs/a.txt", b"x");
    let built = Manifest::build_with_limits(dir.path(), entries(2)).expect("at the limit");
    assert!(matches!(built, Build::Ready(_)));
    let err = Manifest::build_with_limits(dir.path(), entries(1)).unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
}

#[test]
fn a_folder_whose_manifest_is_over_the_byte_limit_fails_without_naming_anything() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "secret/report.txt", b"hello");
    let exact = ready(dir.path()).manifest.encode().len() as u64;
    let bytes = |max_bytes: u64| Limits {
        max_bytes,
        ..Limits::default()
    };
    let built = Manifest::build_with_limits(dir.path(), bytes(exact)).expect("at the limit");
    assert!(matches!(built, Build::Ready(_)));

    let err = Manifest::build_with_limits(dir.path(), bytes(exact - 1)).unwrap_err();
    let Error::Policy(message) = err else {
        panic!("expected a policy error, got {err}");
    };
    assert!(message.contains(&format!("{exact} bytes")), "{message}");
    assert!(
        message.contains(&format!("at most {}", exact - 1)),
        "{message}"
    );
    assert!(!message.contains("secret"), "{message}");
}
