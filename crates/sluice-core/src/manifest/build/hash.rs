//! The BLAKE3 hash of one file, taken so that it names a version of the file that existed.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use super::meta;

/// What hashing a file found out about it.
#[derive(Debug)]
pub(super) struct Hashed {
    pub root: [u8; 32],
    pub size: u64,
    pub mtime_ms: u64,
    pub mode: u32,
}

#[derive(Debug)]
pub(super) enum HashFault {
    /// The file could not be opened or read.
    Unreadable(io::ErrorKind),
    /// Its size or modification time changed while it was read, or it gave a number of bytes
    /// other than its size, so the hash is of no version of it.
    Changed,
}

/// Counts what passes through, to compare with the size the file reported.
struct Counting<R> {
    inner: R,
    bytes: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}

pub(super) fn hash_file(path: &Path) -> Result<Hashed, HashFault> {
    hash_file_with(path, || {})
}

/// Hashes `path`. `between` runs after the last byte is read and before the second look at the
/// file, which is the window a writer can use. A test puts its write there.
///
/// The size and time are taken from the open handle and not from the path, so a file swapped
/// for another between the walk and this read is the file that is hashed and described.
pub(super) fn hash_file_with(path: &Path, between: impl FnOnce()) -> Result<Hashed, HashFault> {
    let unreadable = |e: io::Error| HashFault::Unreadable(e.kind());
    let file = File::open(path).map_err(unreadable)?;
    let before = file.metadata().map_err(unreadable)?;
    let mut reader = Counting {
        inner: file,
        bytes: 0,
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut reader).map_err(unreadable)?;
    between();
    let after = reader.inner.metadata().map_err(unreadable)?;
    let unchanged = before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && reader.bytes == before.len();
    if !unchanged {
        return Err(HashFault::Changed);
    }
    Ok(Hashed {
        root: *hasher.finalize().as_bytes(),
        size: before.len(),
        mtime_ms: meta::mtime_ms(&before),
        mode: meta::mode(&before),
    })
}
