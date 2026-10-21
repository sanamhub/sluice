//! Reading one file into a BLAKE3 hasher, so that what was hashed is a version of the file that
//! existed. The hasher is the file's own for a blob, and its pack's for a packed file.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use super::meta;

/// What reading a file found out about it, besides its bytes.
#[derive(Debug)]
pub(super) struct Facts {
    pub size: u64,
    pub mtime_ms: u64,
    pub mode: u32,
}

#[derive(Debug)]
pub(super) enum HashFault {
    /// The file could not be opened or read.
    Unreadable(io::ErrorKind),
    /// It is no longer a regular file, its size or modification time changed while it was read,
    /// or it gave a number of bytes other than its size, so the hash is of no version of it.
    Changed,
}

/// A file that is open and was a regular file when it was opened, with its first look.
#[derive(Debug)]
pub(super) struct Opened {
    file: File,
    before: Metadata,
    /// On Windows, the handle of a reparse point that is not a link, held while `file` is read
    /// through its filter, so the path cannot be renamed or deleted in between.
    _held: Option<File>,
}

impl Opened {
    /// The size from the open handle. It decides whether the file is packed.
    pub(super) fn size(&self) -> u64 {
        self.before.len()
    }
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

/// Opens `path` and takes the first look at it. The walk saw a regular file here, so a handle to
/// anything else means the folder changed.
pub(super) fn open(path: &Path) -> Result<Opened, HashFault> {
    let file = open_no_follow(path)?;
    let before = file
        .metadata()
        .map_err(|e| HashFault::Unreadable(e.kind()))?;
    #[cfg(windows)]
    if let windows::Handle::ReparsePoint = windows::classify(&before) {
        return windows::through_filter(path, file);
    }
    if !before.is_file() {
        return Err(HashFault::Changed);
    }
    Ok(Opened {
        file,
        before,
        _held: None,
    })
}

/// Reads all of `opened` into `hasher`. `between` runs after the last byte is read and before the
/// second look at the file, which is the window a writer can use. A test puts its write there.
///
/// The size and time are taken from the open handle and not from the path, so a file swapped for
/// another between the walk and this read is the file that is hashed and described.
pub(super) fn read_into(
    opened: Opened,
    hasher: &mut blake3::Hasher,
    between: impl FnOnce(),
) -> Result<Facts, HashFault> {
    let unreadable = |e: io::Error| HashFault::Unreadable(e.kind());
    // `_held` is a binding and not `_`, so the first handle stays open until the second look.
    let Opened {
        file,
        before,
        _held,
    } = opened;
    let mut reader = Counting {
        inner: file,
        bytes: 0,
    };
    hasher.update_reader(&mut reader).map_err(unreadable)?;
    between();
    let after = reader.inner.metadata().map_err(unreadable)?;
    let unchanged = before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && reader.bytes == before.len();
    if !unchanged {
        return Err(HashFault::Changed);
    }
    Ok(Facts {
        size: before.len(),
        mtime_ms: meta::mtime_ms(&before),
        mode: meta::mode(&before),
    })
}

/// Opens `path` without following a link at its last component. Another user who can write into
/// the folder could swap the file for a link after the walk, and a plain open would then hash
/// and send whatever the link points at that this user can read, such as an SSH key.
fn open_no_follow(path: &Path) -> Result<File, HashFault> {
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

/// A link is opened as itself, and its metadata then says it is not a regular file. Without
/// `FILE_SHARE_DELETE`, nobody can rename or delete the file while the handle is open.
#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    options
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(windows::SHARE);
}

#[cfg(windows)]
mod windows;
