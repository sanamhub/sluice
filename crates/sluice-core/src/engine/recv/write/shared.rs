//! The store copies a receive may have handed to the output, and the check that a kept store can
//! still be trusted for them.

use std::{fs, path::Path};

use iroh_blobs::Hash;

use super::super::storage;
use super::file;
use crate::Error;

/// The files that took the store's bytes by link. A hard link is one inode under two names, so a
/// mode or a time set on the installed file is set on the store's copy too, and an edit to the user's
/// file changes the blob the store would resume from. A store that is kept after a failure is given
/// its own copy of each before anything reads it.
#[derive(Default)]
pub(super) struct Shared {
    files: Vec<(Hash, std::path::PathBuf)>,
    /// The store folder the copies belong to. Empty for a `Shared` that was never given one.
    partial: std::path::PathBuf,
}

impl Shared {
    pub(super) fn new(partial: &Path) -> Self {
        Self {
            files: Vec::new(),
            partial: partial.to_path_buf(),
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
    /// The copies are made on a blocking thread: a blob can be gigabytes, and an interactive
    /// command that waits for one must not stop the runtime that reports what it is doing.
    ///
    /// A copy that fails leaves the store sharing bytes with a file the user may edit, so the store
    /// cannot be resumed from; the caller discards it rather than keep one it cannot trust, and the
    /// store's next claim repairs what a cancellation left.
    pub(super) async fn release(&self) -> Result<(), Error> {
        for (hash, target) in &self.files {
            let source = storage::data_path(&self.partial, hash);
            if !same_file(target, &source) {
                continue;
            }
            let size = fs::metadata(&source)
                .map_err(|e| storage::storage("read", &source, e))?
                .len();
            // The copy is a second name for bytes that are already here, so the volume needs one
            // more copy of the blob and not two. Without the room the copy would fail halfway and
            // leave the store holding part of a name.
            let free = storage::available_bytes(&self.partial)?;
            if free < size.saturating_add(COPY_SLACK) {
                return Err(Error::Verify(format!(
                    "the store could not be given its own bytes for the file at {}: {} bytes are \
                     free where it needs about {size}; the store cannot be resumed from",
                    target.display(),
                    free
                )));
            }
            // A copy beside it, renamed over the store's name: the store keeps an inode of its own
            // and the installed file keeps the one it has.
            let copy = source.with_extension("store");
            let from = source.clone();
            let to = copy.clone();
            let done = tokio::task::spawn_blocking(move || {
                fs::copy(&from, &to).and_then(|_| fs::rename(&to, &from))
            })
            .await
            .map_err(|e| {
                Error::Verify(format!(
                    "the copy a block of the store's bytes ran on stopped: {e}; the store cannot \
                     be resumed from"
                ))
            })?;
            if let Err(e) = done {
                let _ = fs::remove_file(source.with_extension("store"));
                return Err(Error::Verify(format!(
                    "the store could not be given its own bytes for the file at {}: {e}; it cannot \
                     be resumed from",
                    target.display()
                )));
            }
        }
        Ok(())
    }
}

/// The room a copy of a blob needs beyond its own size, for the file system's bookkeeping.
const COPY_SLACK: u64 = 1 << 20;

/// Whether the store's data file for `hash` still holds the blob's bytes. A blob whose data file
/// has one name was written by the store and verified as it arrived; one with more names is shared
/// with an output file, which the user can edit between two runs. That copy is hashed before it is
/// used, because the store's own bookkeeping cannot see an edit made through another name.
pub(super) fn store_holds(partial: &Path, hash: Hash, size: u64) -> Result<bool, Error> {
    let source = storage::data_path(partial, &hash);
    let meta = match fs::symlink_metadata(&source) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(storage::storage("read", &source, e)),
    };
    if storage::name_count(&source, &meta)? <= 1 {
        return Ok(true);
    }
    file::same_blob(&source, hash, size)
}

/// Whether two names are the same file. The platform's own file identity answers it without reading
/// either, so a large file is not hashed to find out.
pub(super) fn same_file(one: &Path, other: &Path) -> bool {
    match (storage::identify(one), storage::identify(other)) {
        (Ok(Some(one)), Ok(Some(other))) => one == other,
        _ => false,
    }
}
