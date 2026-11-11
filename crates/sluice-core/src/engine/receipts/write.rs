//! Writing the receipt a receive asked for, with the care an output file gets: a link at the name
//! is never followed, and a file that is already there is not replaced without `--force`.
//!
//! The file is created exclusively, so the open itself refuses a name that exists. A `--force`
//! replacement removes the name first, which removes a link as a link and never writes through one.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use crate::Error;
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
/// `force`, so a file that appeared meanwhile is a refusal from the exclusive create rather than a
/// name this writes through.
pub(in crate::engine) fn write_file(
    path: &Path,
    receipt: &Receipt,
    force: bool,
) -> Result<(), Error> {
    let label = shown(&path.display().to_string());
    if force {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                return Err(Error::Policy(format!(
                    "{label} is a folder, and a receipt is one file"
                )));
            }
            Ok(_) => fs::remove_file(path)
                .map_err(|e| Error::Storage(format!("cannot replace {label}: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Storage(format!("cannot read {label}: {e}"))),
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| Error::Storage(format!("cannot write {label}: {e}")))?;
    file.write_all(format!("{}\n", receipt.encode()).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| Error::Storage(format!("cannot write {label}: {e}")))
}

#[cfg(test)]
mod tests {
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
}
