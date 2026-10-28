//! Giving a store its own bytes back when a command stopped before it could.
//!
//! A finished file may be a hard link of the store's data file: that is what makes a large receive
//! cost one copy instead of two. A command that fails while keeping its store copies those bytes
//! back, but a command that is cancelled has no moment to copy anything, so it leaves the store
//! sharing an inode with the output. This is where the next command puts that right, before the
//! store is opened.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use iroh_blobs::{Hash, store::fs::options::Options};

use super::available_bytes;
use crate::Error;

/// How much room a copy needs beyond its own size. The copy is a second name for bytes that are
/// already on the volume, so the volume needs one more copy of the blob and a little slack.
const COPY_SLACK: u64 = 1 << 20;

/// The data file of `hash` in the store at `partial`.
pub(crate) fn data_path(partial: &Path, hash: &Hash) -> PathBuf {
    Options::new(partial).path.data_path(hash)
}

/// Puts a store that a cancelled command left sharing bytes with the output back on its own.
/// Called before the store is opened, so nothing holds a data file while it is replaced: on
/// Windows a rename over a file the store has open would fail, and a store whose bytes could not be
/// given back is refused where they are used rather than trusted.
///
/// Every step is best effort. A store that cannot be repaired keeps the bytes it has, and the check
/// that reads a blob shared with the output still refuses one whose bytes no longer match.
pub async fn repair(partial: &Path) {
    let data = Options::new(partial).path.data_path;
    let Ok(entries) = fs::read_dir(&data) else {
        return;
    };
    let mut shared = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A copy that a killed command left halfway. The name is derived from the data file, so it
        // is never a blob of its own.
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
}

/// Copies the store's bytes to a name of their own, then puts that name in place of the shared one.
/// Nothing is copied when the bytes no longer hash to the root they are filed under: those are the
/// user's bytes now, and the store's name for them cannot be trusted.
async fn give_back(partial: &Path, path: PathBuf, hash: Hash) {
    let size = match fs::metadata(&path) {
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
        if !hashes_to(&source, hash)? {
            return Ok::<bool, std::io::Error>(false);
        }
        fs::copy(&source, &target)?;
        fs::rename(&target, &source)?;
        Ok(true)
    })
    .await;
    if !matches!(done, Ok(Ok(true))) {
        let _ = fs::remove_file(&copy);
    }
}

/// Whether the bytes of `path` hash to `hash`. A file that cannot be read is not one this can
/// answer for, so it is left as it is.
fn hashes_to(path: &Path, hash: Hash) -> std::io::Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finalize().as_bytes() == hash.as_bytes());
        }
        hasher.update(&buffer[..read]);
    }
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
