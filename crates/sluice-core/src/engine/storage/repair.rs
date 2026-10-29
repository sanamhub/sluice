//! Giving a store its own bytes back when a command stopped before it could.
//!
//! A finished file may be a hard link of the store's data file: that is what makes a large receive
//! cost one copy instead of two. A command that fails while keeping its store copies those bytes
//! back, but a command that is cancelled has no moment to copy anything, so it leaves the store
//! sharing an inode with the output. This is where the next command puts that right, before the
//! store is opened.
//!
//! The folder is opened once, and the listing, the copy and the rename happen through that handle
//! or under the checks made on it. A folder renamed away after the check would otherwise be
//! repaired through names another user chose, and a link at its `data` folder would be followed.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use iroh_blobs::{Hash, store::fs::options::Options};

use super::{FolderId, identify};
#[cfg(not(unix))]
use super::{available_bytes, storage};
use crate::Error;

#[cfg(not(any(unix, windows)))]
mod other;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(not(any(unix, windows)))]
use other::Dir;
#[cfg(unix)]
use unix::Dir;
#[cfg(windows)]
use windows::Dir;

#[cfg(test)]
mod tests;

/// How much room a copy needs beyond its own size. The copy is a second name for bytes that are
/// already on the volume, so the volume needs one more copy of the blob and a little slack.
const COPY_SLACK: u64 = 1 << 20;

/// The data file of `hash` in the store at `partial`.
pub(crate) fn data_path(partial: &Path, hash: &Hash) -> PathBuf {
    Options::new(partial).path.data_path(hash)
}

/// Why a store was not touched: the folder under its name is not the folder that was checked.
pub(crate) fn changed(partial: &Path) -> Error {
    Error::Policy(format!(
        "the partial store folder {} changed while it was opened; look at what is there",
        partial.display()
    ))
}

/// Why a store was not touched: what a store keeps its data files in is not a folder of this tree.
fn alien(partial: &Path) -> Error {
    Error::Policy(format!(
        "the partial store folder {} has a link or a file where its data folder belongs; look at \
         what is there",
        partial.display()
    ))
}

/// Puts a store that a cancelled command left sharing bytes with the output back on its own.
/// Called before the store is opened, so nothing holds a data file while it is replaced: on
/// Windows a rename over a file the store has open would fail, and a store whose bytes could not be
/// given back is refused where they are used rather than trusted.
///
/// `expected` is the identity the caller read for this folder a moment ago. It is checked here,
/// before the first file is looked at, because another user who can write in the output can rename
/// this folder away and put one of their own under the name: repairing that one would copy through
/// names they chose, and the caller's own check comes only after the store is opened. The same
/// identity is checked again against the opened folder, so the files are looked at in the folder
/// that passed the check.
///
/// Every step after that is best effort. A store that cannot be repaired keeps the bytes it has, and
/// the check that reads a blob shared with the output still refuses one whose bytes no longer match.
pub async fn repair(partial: &Path, expected: Option<FolderId>) -> Result<(), Error> {
    if identify(partial)? != expected {
        return Err(changed(partial));
    }
    let folder = partial.to_path_buf();
    // The open can wait for another program that holds the folder, and the sweep waits for files a
    // scanner holds. Those waits belong on a blocking thread, and one thread does the whole pass.
    tokio::task::spawn_blocking(move || repair_here(&folder, expected))
        .await
        .map_err(|error| Error::Storage(format!("cannot repair {}: {error}", partial.display())))?
}

/// One pass over the store: open its folder, check that it is the folder the caller checked, sweep
/// it, and on the platform whose handles cannot list a folder, check it again by name.
fn repair_here(partial: &Path, expected: Option<FolderId>) -> Result<(), Error> {
    let Some(dir) = Dir::open(partial)? else {
        return Ok(());
    };
    if dir.identity()? != expected {
        return Err(changed(partial));
    }
    dir.sweep(partial)?;
    recheck(partial, expected)
}

/// Copied and hashed in one pass, so the hash is of the bytes that went into the target and not of
/// the bytes that were in the source when the copy began. An edit through a hard link between two
/// passes would otherwise be installed as the blob.
fn copy_and_hash(mut from: impl Read, mut to: impl Write, hash: Hash) -> io::Result<bool> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = from.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        to.write_all(&buffer[..read])?;
    }
    Ok(hasher.finalize().as_bytes() == hash.as_bytes())
}

/// A second identity check after the repair, on the platform whose handles cannot carry the whole
/// listing: the folder named now has to be the folder that was checked and repaired.
#[cfg(windows)]
fn recheck(partial: &Path, expected: Option<FolderId>) -> Result<(), Error> {
    if identify(partial)? != expected {
        return Err(changed(partial));
    }
    Ok(())
}

#[cfg(not(windows))]
fn recheck(_partial: &Path, _expected: Option<FolderId>) -> Result<(), Error> {
    Ok(())
}

/// The sweep through paths, for the platform that cannot list a folder from its handle. The caller
/// has already opened `data` without following a link and made sure it is a real folder.
#[cfg(not(unix))]
fn sweep_paths(data: &Path, partial: &Path) -> Result<(), Error> {
    let entries = match fs::read_dir(data) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage("read", data, error)),
    };
    let mut shared = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A copy that a killed command left halfway. The name is derived from the data file, so it
        // is never a blob of its own. Removing it removes the entry and never what a link leads to,
        // so a link planted under this name is taken away rather than followed.
        if name.ends_with(".store") {
            let _ = fs::remove_file(&path);
            continue;
        }
        let Some(hex) = name.strip_suffix(".data") else {
            continue;
        };
        let Ok(hash) = hex.parse::<Hash>() else {
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        if name_count(&path, &meta).unwrap_or(0) > 1 {
            shared.push((path, hash, meta.len()));
        }
    }
    for (path, hash, size) in shared {
        give_back_path(&path, hash, size, partial);
    }
    Ok(())
}

/// Gives one path-based store file its own bytes back, and removes the copy if it is not the blob.
#[cfg(not(unix))]
fn give_back_path(path: &Path, hash: Hash, size: u64, partial: &Path) {
    if available_bytes(partial).is_ok_and(|free| free < size.saturating_add(COPY_SLACK)) {
        return;
    }
    let copy = path.with_extension("store");
    let done = copy_then_rename(path, &copy, hash);
    if !matches!(done, Ok(true)) {
        let _ = fs::remove_file(&copy);
    }
}

#[cfg(not(unix))]
fn copy_then_rename(source: &Path, copy: &Path, hash: Hash) -> io::Result<bool> {
    let mut from = open_existing(source)?;
    let mut to = open_new(copy)?;
    if !copy_and_hash(&mut from, &mut to, hash)? {
        return Ok(false);
    }
    to.sync_all()?;
    drop(to);
    fs::rename(copy, source)?;
    Ok(true)
}

/// Opens the source for reading without following a link at its name. The store writes a data file
/// itself and never a link, so a link there is someone else's file and none of the store's bytes.
#[cfg(windows)]
fn open_existing(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_existing(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

/// Opens a name that must not be there yet, without following a link at it, so the copy can only
/// ever be a new file this call made.
#[cfg(windows)]
fn open_new(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_new(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// How many names the file at `path` has on its volume. A file this receive linked to the output
/// has two; a store that shares it with anything else is treated the same way.
#[cfg(unix)]
pub(crate) fn name_count(_path: &Path, meta: &fs::Metadata) -> Result<u64, Error> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.nlink())
}

#[cfg(windows)]
pub(crate) fn name_count(path: &Path, _meta: &fs::Metadata) -> Result<u64, Error> {
    crate::home::private_windows::link_count(path)
        .map(u64::from)
        .map_err(|e| Error::Storage(format!("cannot read {}: {e}", path.display())))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn name_count(_path: &Path, _meta: &fs::Metadata) -> Result<u64, Error> {
    // No call for it: assume the worst, so the blob is treated as one that is shared.
    Ok(2)
}
