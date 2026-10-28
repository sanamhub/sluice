//! Giving a store its own bytes back when a command stopped before it could.
//!
//! A finished file may be a hard link of the store's data file: that is what makes a large receive
//! cost one copy instead of two. A command that fails while keeping its store copies those bytes
//! back, but a command that is cancelled has no moment to copy anything, so it leaves the store
//! sharing an inode with the output. This is where the next command puts that right, before the
//! store is opened.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use iroh_blobs::{Hash, store::fs::options::Options};

use super::{FolderId, available_bytes, identify};
use crate::Error;

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

/// Puts a store that a cancelled command left sharing bytes with the output back on its own.
/// Called before the store is opened, so nothing holds a data file while it is replaced: on
/// Windows a rename over a file the store has open would fail, and a store whose bytes could not be
/// given back is refused where they are used rather than trusted.
///
/// `expected` is the identity the caller read for this folder a moment ago. It is checked here,
/// before the first file is looked at, because another user who can write in the output can rename
/// this folder away and put one of their own under the name: repairing that one would copy through
/// names they chose, and the caller's own check comes only after the store is opened.
///
/// Every step after that is best effort. A store that cannot be repaired keeps the bytes it has, and
/// the check that reads a blob shared with the output still refuses one whose bytes no longer match.
pub async fn repair(partial: &Path, expected: Option<FolderId>) -> Result<(), Error> {
    if identify(partial)? != expected {
        return Err(changed(partial));
    }
    let data = Options::new(partial).path.data_path;
    let Ok(entries) = fs::read_dir(&data) else {
        return Ok(());
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
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if name_count(&path, &meta).unwrap_or(0) > 1 {
            shared.push((path, hash));
        }
    }
    for (path, hash) in shared {
        give_back(partial, path, hash).await;
    }
    Ok(())
}

/// Copies the store's bytes to a name of their own, then puts that name in place of the shared one.
/// Nothing is installed when the bytes do not hash to the root they are filed under: those are the
/// user's bytes now, and the store's name for them cannot be trusted.
async fn give_back(partial: &Path, path: PathBuf, hash: Hash) {
    let size = match fs::symlink_metadata(&path) {
        Ok(meta) => meta.len(),
        Err(_) => return,
    };
    if available_bytes(partial).is_ok_and(|free| free < size.saturating_add(COPY_SLACK)) {
        return;
    }
    let copy = path.with_extension("store");
    let source = path.clone();
    let target = copy.clone();
    let done = tokio::task::spawn_blocking(move || {
        // The copy is opened new and without following a link, so a link planted at its name after
        // the sweep above fails the open rather than receiving the store's bytes.
        if !copy_own(&source, &target, hash)? {
            return Ok::<bool, std::io::Error>(false);
        }
        fs::rename(&target, &source)?;
        Ok(true)
    })
    .await;
    if !matches!(done, Ok(Ok(true))) {
        let _ = fs::remove_file(&copy);
    }
}

/// Copies `source` to `target`, hashing the bytes as they are written, and says whether the file
/// that was written is the blob of `hash`.
///
/// The hash covers the bytes that went into the target and not the bytes that were in the source a
/// moment earlier: the source may be the user's file through a hard link, and an edit through it
/// between a hash and a copy would otherwise be installed as the blob. A caller that gets `true`
/// has a file at `target` whose own bytes hash to the root, whatever happened to the source.
fn copy_own(source: &Path, target: &Path, hash: Hash) -> std::io::Result<bool> {
    let mut from = open_existing(source)?;
    let mut to = open_new(target)?;
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
    to.sync_all()?;
    Ok(hasher.finalize().as_bytes() == hash.as_bytes())
}

/// Opens the source for reading without following a link at its name. The store writes a data file
/// itself and never a link, so a link there is someone else's file and none of the store's bytes.
#[cfg(unix)]
fn open_existing(path: &Path) -> std::io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(fs::File::from(fd))
}

#[cfg(windows)]
fn open_existing(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_existing(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

/// Opens a name that must not be there yet, without following a link at it, so the copy can only
/// ever be a new file this call made.
#[cfg(unix)]
fn open_new(path: &Path) -> std::io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    Ok(fs::File::from(fd))
}

#[cfg(windows)]
fn open_new(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_new(path: &Path) -> std::io::Result<fs::File> {
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

#[cfg(test)]
mod tests {
    use super::copy_own;

    /// The bytes a copy is judged by are the ones it wrote. A source that holds another blob gives
    /// a copy that is not the blob of the root, and nothing is installed from it.
    #[test]
    fn a_copy_is_judged_by_the_bytes_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::write(&source, b"one").unwrap();
        let root = iroh_blobs::Hash::new(b"one");
        assert!(copy_own(&source, &target, root).unwrap());
        assert_eq!(std::fs::read(&target).unwrap(), b"one");

        std::fs::remove_file(&target).unwrap();
        let other = iroh_blobs::Hash::new(b"another");
        assert!(
            !copy_own(&source, &target, other).unwrap(),
            "a copy that is not the blob was judged to be one"
        );
    }

    /// The copy's name is opened new and without following a link, so bytes are never written
    /// through a link that someone planted under it.
    #[cfg(unix)]
    #[test]
    fn a_copy_never_writes_through_a_link_at_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let outside = dir.path().join("outside");
        let target = dir.path().join("target");
        std::fs::write(&source, b"one").unwrap();
        std::fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();

        let root = iroh_blobs::Hash::new(b"one");
        assert!(copy_own(&source, &target, root).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
    }
}
