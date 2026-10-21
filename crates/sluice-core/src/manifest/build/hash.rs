//! The BLAKE3 hash of one file, taken so that it names a version of the file that existed.

use std::fs::{self, File, OpenOptions};
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
/// for another between the walk and this read is the file that is hashed and described. The
/// walk saw a regular file here, so a handle to anything else means the folder changed.
pub(super) fn hash_file_with(path: &Path, between: impl FnOnce()) -> Result<Hashed, HashFault> {
    let unreadable = |e: io::Error| HashFault::Unreadable(e.kind());
    let file = open(path)?;
    let before = file.metadata().map_err(unreadable)?;
    if !before.is_file() {
        return Err(HashFault::Changed);
    }
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

/// Opens `path` without following a link at its last component. Another user who can write into
/// the folder could swap the file for a link after the walk, and a plain open would then hash
/// and send whatever the link points at that this user can read, such as an SSH key.
fn open(path: &Path) -> Result<File, HashFault> {
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    options.open(path).map_err(|e| {
        // The open of a link fails here (`ELOOP` on Unix), and so does the open of a folder on
        // Windows. Either one is a path that was a regular file during the walk.
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => HashFault::Changed,
            _ => HashFault::Unreadable(e.kind()),
        }
    })
}

/// `O_NOFOLLOW` refuses a link. `O_NONBLOCK` makes the open of a FIFO return at once, where a
/// plain open waits for a writer that may never come. It changes nothing for a regular file.
#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

/// A link is opened as itself, and its metadata then says it is not a regular file.
#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}
