//! Writing the receipt a receive asked for, with the care an output file gets: a link at the name
//! is never followed, and a file that is already there is not replaced without `--force`.
//!
//! The receipt is written and synced to a temporary name beside the file, and then put in place by
//! one rename. Without `--force` that rename is the kind that refuses a name that exists
//! (`RENAME_NOREPLACE` on Linux, `RENAME_EXCL` on macOS, `MoveFileExW` without the replace flag on
//! Windows), so a file that appeared since the check is never overwritten. With `--force` it is a
//! plain rename, which replaces the name, a link included, as a link. A file system that cannot
//! refuse a name in a rename (an old kernel, FUSE, SMB) gets the old way instead: an exclusive
//! create at the final path, written and synced there. A write that fails leaves the temporary file
//! removed and the name as it was, so no half-written receipt is at the path a verifier reads.
//! Nothing here links a file.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::Error;
use crate::engine::storage::{is_unsupported, rename_exclusive};
use crate::manifest::path::shown;
use crate::receipt::Receipt;

mod clash;
#[cfg(test)]
mod tests;

pub(in crate::engine) use clash::refuse_under_out;

/// The longest file name, in bytes, that every file system a receipt can go to takes.
const NAME_MAX: usize = 255;
/// How many random names are tried for the temporary file before giving up.
const NAME_TRIES: usize = 8;

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
        Err(e) if e.kind() == io::ErrorKind::NotFound => parent_ready(path, &label),
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

/// Writes `receipt` to `path` as one line, as the module says. The caller has already passed
/// [`ready`] for the same `force`, so a file that appeared meanwhile is a refusal from the rename
/// and not a name this replaces. Once the file is in place the result is `Ok`; a temporary file that
/// could not be removed afterwards comes back as a message for the caller to show as a warning.
pub(in crate::engine) fn write_file(
    path: &Path,
    receipt: &Receipt,
    force: bool,
) -> Result<Option<String>, Error> {
    let label = shown(&path.display().to_string());
    if force && fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
        return Err(Error::Policy(format!(
            "{label} is a folder, and a receipt is one file"
        )));
    }
    write_beside(path, receipt, force)
        .map_err(|e| Error::Storage(format!("cannot write {label}: {e}")))
}

/// The temporary file, the rename, and the fallback for a file system that cannot refuse a name.
fn write_beside(path: &Path, receipt: &Receipt, force: bool) -> io::Result<Option<String>> {
    let line = format!("{}\n", receipt.encode());
    let (temporary, mut file) = create_temporary(path)?;
    let written = file
        .write_all(line.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let placed = written.and_then(|()| {
        if force {
            fs::rename(&temporary, path)
        } else {
            rename_exclusive(&temporary, path)
        }
    });
    match placed {
        Ok(()) => {
            sync_parent(path);
            Ok(None)
        }
        Err(e) if !force && is_unsupported(&e) => {
            let leftover = remove(&temporary);
            write_in_place(path, &line)?;
            Ok(leftover)
        }
        Err(e) => match remove(&temporary) {
            None => Err(e),
            Some(left) => Err(io::Error::new(e.kind(), format!("{e}; {left}"))),
        },
    }
}

/// The old way, for a file system that cannot refuse a name in a rename: an exclusive create at the
/// final path, written and synced there. A file this created and could not finish is removed.
fn write_in_place(path: &Path, line: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let written = file
        .write_all(line.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written {
        let _ = fs::remove_file(path);
        return Err(e);
    }
    sync_parent(path);
    Ok(())
}

/// Removes the temporary file, and says in a sentence why it is still there when that fails.
fn remove(temporary: &Path) -> Option<String> {
    match fs::remove_file(temporary) {
        Ok(()) => None,
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => Some(format!(
            "the temporary file {} could not be removed: {e}",
            shown(&temporary.display().to_string())
        )),
    }
}

/// Makes the rename durable. The receipt is already in place, so a failure here changes nothing a
/// reader sees and is not reported.
#[cfg(unix)]
fn sync_parent(path: &Path) {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let _ = fs::File::open(parent).and_then(|folder| folder.sync_all());
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) {}

/// Creates a temporary file beside `path` under a random name, and tries another name when one is
/// taken.
fn create_temporary(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..NAME_TRIES {
        let temporary = path.with_file_name(temporary_name(path)?);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// A name for the temporary file: a dot, the receipt's name, 16 random hex digits and `.tmp`. The
/// receipt's name is cut at a character boundary so the whole name stays within [`NAME_MAX`] bytes.
fn temporary_name(path: &Path) -> io::Result<String> {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let digits: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let suffix = format!(".{digits}.tmp");
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut keep = name.len().min(NAME_MAX - 1 - suffix.len());
    while !name.is_char_boundary(keep) {
        keep -= 1;
    }
    Ok(format!(".{}{suffix}", &name[..keep]))
}
