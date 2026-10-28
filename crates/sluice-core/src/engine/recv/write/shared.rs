//! The store copies a receive may have handed to the output, and the check that a kept store can
//! still be trusted for them.

use std::{fs, path::Path};

use iroh_blobs::{Hash, store::fs::options::Options};

use super::super::storage;
use super::file;
use crate::Error;

/// The data file of `hash` in the store at `partial`, in the layout `FsStore::load` makes.
pub(super) fn data_path(partial: &Path, hash: &Hash) -> std::path::PathBuf {
    Options::new(partial).path.data_path(hash)
}

/// The files that took the store's bytes by link. A hard link is one inode under two names, so a
/// mode or a time set on the installed file is set on the store's copy too, and an edit to the user's
/// file changes the blob the store would resume from. A store that is kept after a failure is given
/// its own copy of each before anything reads it.
#[derive(Default)]
pub(super) struct Shared {
    files: Vec<(Hash, std::path::PathBuf)>,
    /// The store folder the copies belong to. Empty for a `Shared` that was never given one.
    partial: std::path::PathBuf,
    /// Set once the store is no longer kept, so a drop has nothing left to protect.
    settled: bool,
}

impl Shared {
    pub(super) fn new(partial: &Path) -> Self {
        Self {
            files: Vec::new(),
            partial: partial.to_path_buf(),
            settled: false,
        }
    }

    /// Records that `target` took the bytes of `hash` by link.
    pub(super) fn took(&mut self, hash: Hash, target: std::path::PathBuf) {
        self.files.push((hash, target));
    }

    /// Gives the store its own bytes for every file that still shares an inode with it. Called when
    /// the transfer fails and the store is kept: on the success path the store is removed, and the
    /// link was what made one large file cost one copy rather than two.
    ///
    /// A copy that fails leaves the store sharing bytes with a file the user may edit, so the store
    /// cannot be resumed from; the caller discards it rather than keep one it cannot trust.
    pub(super) fn release(&self) -> Result<(), Error> {
        for (hash, target) in &self.files {
            let source = data_path(&self.partial, hash);
            if !same_file(target, &source) {
                continue;
            }
            // A copy beside it, renamed over the store's name: the store keeps an inode of its own
            // and the installed file keeps the one it has.
            let copy = source.with_extension("store");
            let copied = fs::copy(&source, &copy).and_then(|_| fs::rename(&copy, &source));
            if let Err(e) = copied {
                let _ = fs::remove_file(&copy);
                return Err(Error::Verify(format!(
                    "the store could not be given its own bytes for the file at {}: {e}; it cannot \
                     be resumed from",
                    target.display()
                )));
            }
        }
        Ok(())
    }

    /// Marks the store as one that will not be kept, so dropping this releases nothing.
    pub(super) fn settled(&mut self) {
        self.settled = true;
    }
}

impl Drop for Shared {
    /// A cancelled transfer drops this where no error can be returned. The copy is best effort here;
    /// a store that keeps a link is refused by the check before its bytes are exported again, so an
    /// interrupted run cannot make a resume trust what the user wrote into a received file.
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.release();
        }
    }
}

/// Whether the store's data file for `hash` still holds the blob's bytes. A blob whose data file
/// has one name was written by the store and verified as it arrived; one with more names is shared
/// with an output file, which the user can edit between two runs. That copy is hashed before it is
/// used, because the store's own bookkeeping cannot see an edit made through another name.
pub(super) fn store_holds(partial: &Path, hash: Hash, size: u64) -> Result<bool, Error> {
    let source = data_path(partial, &hash);
    let meta = match fs::symlink_metadata(&source) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(storage::storage("read", &source, e)),
    };
    if name_count(&source, &meta)? <= 1 {
        return Ok(true);
    }
    file::same_blob(&source, hash, size)
}

/// The number of names the data file has on its volume. A file this receive linked to the output
/// has two; a store that shares it with anything else is treated the same way.
#[cfg(unix)]
fn name_count(_path: &Path, meta: &fs::Metadata) -> Result<u64, Error> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.nlink())
}

#[cfg(windows)]
fn name_count(path: &Path, _meta: &fs::Metadata) -> Result<u64, Error> {
    crate::home::private_windows::link_count(path)
        .map(u64::from)
        .map_err(|e| Error::Storage(format!("cannot read {}: {e}", path.display())))
}

#[cfg(not(any(unix, windows)))]
fn name_count(_path: &Path, _meta: &fs::Metadata) -> Result<u64, Error> {
    // No call for it: assume the worst, so the blob is hashed before it is used.
    Ok(2)
}

/// Whether two names are the same file. The platform's own file identity answers it without reading
/// either, so a large file is not hashed to find out.
pub(super) fn same_file(one: &Path, other: &Path) -> bool {
    match (storage::identify(one), storage::identify(other)) {
        (Ok(Some(one)), Ok(Some(other))) => one == other,
        _ => false,
    }
}
