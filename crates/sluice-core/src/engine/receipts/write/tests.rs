//! Writing the receipt file: the names it may take, what a failure leaves behind, and the temporary
//! name.

use std::path::Path;

use ed25519_dalek::SigningKey;

use super::{NAME_MAX, ready, temporary_name, write_beside, write_file};
use crate::receipt::{Receipt, commitment};

fn receipt() -> Receipt {
    Receipt::sign(
        &SigningKey::from_bytes(&[7; 32]),
        commitment(&[9; 32]),
        1024,
        1_800_000_000_000,
    )
}

fn names(folder: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    names
}

/// A file that is already there is a policy refusal without `--force`, and the refusal names the
/// path and not what the file holds.
#[test]
fn an_existing_file_is_refused_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    std::fs::write(&path, "kept").unwrap();
    let error = ready(&path, false).unwrap_err();
    assert_eq!(error.kind(), crate::ErrorKind::Policy);
    assert!(error.to_string().contains("receipt.txt"), "{error}");
    ready(&path, true).unwrap();
    write_file(&path, &receipt(), true).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{}\n", receipt().encode())
    );
    assert_eq!(names(dir.path()), ["receipt.txt"]);
}

/// A link at the name is replaced as a link, and the file it pointed at keeps its bytes.
#[cfg(unix)]
#[test]
fn a_link_is_never_written_through() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "untouched").unwrap();
    let link = dir.path().join("receipt.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    write_file(&link, &receipt(), true).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink());
}

/// A folder is not replaced by a receipt, with or without `--force`.
#[test]
fn a_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out");
    std::fs::create_dir(&path).unwrap();
    assert!(ready(&path, true).is_err());
    assert!(write_file(&path, &receipt(), true).is_err());
}

/// The folder the receipt goes into has to exist before the transfer, or the refusal would come
/// after the content had already arrived.
#[test]
fn a_missing_folder_is_refused_before_the_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent").join("receipt.txt");
    let error = ready(&path, false).unwrap_err();
    assert_eq!(error.kind(), crate::ErrorKind::Storage);
}

/// A name that appeared since the check is refused by the rename, the file that is there is as it
/// was, and no temporary file is left beside it.
#[test]
fn a_name_taken_since_the_check_is_kept_and_nothing_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    std::fs::write(&path, "kept").unwrap();
    let error = write_file(&path, &receipt(), false).unwrap_err();
    assert_eq!(error.kind(), crate::ErrorKind::Storage);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
    assert_eq!(names(dir.path()), ["receipt.txt"]);
}

/// A write that succeeds leaves the receipt and nothing else in the folder.
#[test]
fn a_written_receipt_is_the_only_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    assert_eq!(write_file(&path, &receipt(), false).unwrap(), None);
    assert_eq!(names(dir.path()), ["receipt.txt"]);
}

/// The placing fails after the temporary file is written: with `--force` the rename cannot replace a
/// folder that holds something. The folder and what is in it are as they were, and the temporary
/// file is gone.
#[test]
fn a_placing_that_fails_after_the_temporary_file_leaves_everything_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("inside.txt"), "kept").unwrap();
    let before = names(dir.path());
    assert!(write_beside(&path, &receipt(), true).is_err());
    assert_eq!(names(dir.path()), before);
    assert_eq!(names(&path), ["inside.txt"]);
    assert_eq!(
        std::fs::read_to_string(path.join("inside.txt")).unwrap(),
        "kept"
    );
}

/// The temporary name is random, begins with a dot, and stays within 255 bytes for a name of any
/// length, cut at a character and not inside one.
#[test]
fn the_temporary_name_is_random_and_fits() {
    let path = Path::new("receipts").join("é".repeat(300));
    let first = temporary_name(&path).unwrap();
    let second = temporary_name(&path).unwrap();
    assert_ne!(first, second);
    for name in [&first, &second] {
        assert!(name.starts_with('.') && name.ends_with(".tmp"));
        assert!(name.len() <= NAME_MAX, "{} bytes", name.len());
    }
    let short = temporary_name(Path::new("r.txt")).unwrap();
    assert!(short.starts_with(".r.txt.") && short.len() < 40, "{short}");
}
