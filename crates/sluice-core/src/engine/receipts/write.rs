//! Writing the receipt a receive asked for, with the care an output file gets: a link at the name
//! is never followed, and a file that is already there is not replaced without `--force`.
//!
//! The receipt is written to a temporary name beside the file and renamed into place. Without
//! `--force` the rename refuses a name that exists; with it the rename replaces the name, a link
//! included, as a link. A write that fails part way leaves the temporary file removed and the name
//! as it was, so no half-written receipt is ever at the path a verifier will read.

use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

use crate::Error;
use crate::manifest::Manifest;
use crate::manifest::path::shown;
use crate::receipt::Receipt;

/// Checks that the receipt can be written to `path` before a transfer starts, so a clash is
/// reported before any byte moves. `force` allows replacing a file, never a folder.
pub(in crate::engine) fn ready(path: &Path, force: bool) -> Result<(), Error> {
    let label = shown(&path.display().to_string());
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.is_dir() {
                return Err(Error::Policy(format!(
                    "{label} is a folder, and a receipt is one file"
                )));
            }
            if !force {
                return Err(Error::Policy(format!(
                    "{label} is already there; pass --force to replace it"
                )));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => parent_ready(path, &label),
        Err(e) => Err(Error::Storage(format!("cannot read {label}: {e}"))),
    }
}

/// Whether the folder the receipt goes into is there. The open reports a missing folder too, but a
/// refusal before the transfer is worth more than one after it.
fn parent_ready(path: &Path, label: &str) -> Result<(), Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match fs::metadata(parent) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Error::Policy(format!(
            "{} is not a folder, so {label} has no folder to go in",
            shown(&parent.display().to_string())
        ))),
        Err(e) => Err(Error::Storage(format!(
            "cannot read {}: {e}",
            shown(&parent.display().to_string())
        ))),
    }
}

/// Writes `receipt` to `path` as one line. The caller has already passed [`ready`] for the same
/// `force`, so a file that appeared meanwhile is a refusal from the rename rather than a name this
/// replaces.
pub(in crate::engine) fn write_file(
    path: &Path,
    receipt: &Receipt,
    force: bool,
) -> Result<(), Error> {
    let label = shown(&path.display().to_string());
    if force && fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return Err(Error::Policy(format!(
            "{label} is a folder, and a receipt is one file"
        )));
    }
    let failed = |e: std::io::Error| Error::Storage(format!("cannot write {label}: {e}"));
    let temporary = temporary_name(path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(failed)?;
    let written = file
        .write_all(
            format!(
                "{}
",
                receipt.encode()
            )
            .as_bytes(),
        )
        .and_then(|()| file.sync_all());
    drop(file);
    let placed = written.and_then(|()| place(&temporary, path, force));
    if placed.is_err() {
        // A failed write leaves nothing: the temporary file is removed and the name is as it was.
        let _ = fs::remove_file(&temporary);
    }
    placed.map_err(failed)
}

/// Puts the finished file at `path`. With `force` a rename replaces the name, a link included, as a
/// link. Without it the file is linked in, which fails when the name exists, and the temporary name
/// is removed after: a plain rename would replace a file that appeared since the check.
fn place(temporary: &Path, path: &Path, force: bool) -> std::io::Result<()> {
    if force {
        return fs::rename(temporary, path);
    }
    fs::hard_link(temporary, path)?;
    fs::remove_file(temporary)
}

/// A name beside `path` that is not in use: the receipt's name with the process and the time in it.
fn temporary_name(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    path.with_file_name(format!(".{name}.{}.{nanos}.tmp", std::process::id()))
}

/// Refuses a receipt path that is a path the transfer writes under `out`: a file or folder of the
/// manifest, or a folder on the way to one, or `out` itself. The receipt is written after the
/// content, so such a path would be replaced or refused only once everything had arrived. Paths are
/// compared as written, with `.` and `..` folded and case ignored, because a case-insensitive
/// volume names one file by several spellings and a refusal that is too wide costs nothing.
pub(in crate::engine) fn refuse_output_clash(
    receipt: &Path,
    out: &Path,
    manifest: &Manifest,
) -> Result<(), Error> {
    let receipt_key = key(receipt);
    let out_key = key(out);
    let clash = receipt_key == out_key
        || manifest.entries().iter().any(|entry| {
            let destination = entry.path.under(out);
            destination
                .ancestors()
                .take_while(|folder| key(folder) != out_key)
                .any(|folder| key(folder) == receipt_key)
        });
    if clash {
        return Err(Error::Policy(format!(
            "{} is a path this transfer writes under {}, so the receipt cannot go there; choose a              path outside --out",
            shown(&receipt.display().to_string()),
            shown(&out.display().to_string())
        )));
    }
    Ok(())
}

/// A path as a comparison key: absolute, `.` and `..` folded away lexically, lower case.
fn key(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut folded = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other.as_os_str().to_string_lossy().to_lowercase()),
        }
    }
    folded
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ready, write_file};
    use crate::receipt::{Receipt, commitment};
    use ed25519_dalek::SigningKey;

    fn receipt() -> Receipt {
        Receipt::sign(
            &SigningKey::from_bytes(&[7; 32]),
            commitment(&[9; 32]),
            1024,
            1_800_000_000_000,
        )
    }

    /// A file that is already there is a policy refusal without `--force`, and the refusal names
    /// the path and not what the file holds.
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

    /// A write that cannot take the name leaves the file that is there as it was and no temporary
    /// file beside it.
    #[test]
    fn a_failed_write_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.txt");
        std::fs::write(&path, "kept").unwrap();
        let error = write_file(&path, &receipt(), false).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::Storage);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["receipt.txt"]);
    }

    /// A write that succeeds leaves the receipt and nothing else in the folder.
    #[test]
    fn a_written_receipt_is_the_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.txt");
        write_file(&path, &receipt(), false).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["receipt.txt"]);
    }

    /// The comparison folds what a person can spell differently: `.`, `..` and case.
    #[test]
    fn paths_are_compared_without_dots_or_case() {
        assert_eq!(
            super::key(Path::new("inbox/./Sub/../Report.TXT")),
            super::key(Path::new("INBOX/report.txt"))
        );
        assert_ne!(
            super::key(Path::new("inbox/report.txt")),
            super::key(Path::new("inbox/report.txt.bak"))
        );
    }
}
