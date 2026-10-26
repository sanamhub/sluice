//! The one-writer lock of a home.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::Path;

use super::atomic::write_atomic;
use super::storage;
use crate::Error;

/// Takes the exclusive lock on `dir/lock` and records this process's id in `dir/lock.pid`. The
/// lock lives as long as the returned file does.
///
/// The id is in a second file because Windows locks a file against every read by another handle,
/// so a process that was refused could not read the id out of the file that is locked. The
/// operating system drops the lock when the process ends, however it ends, so a lock left by a
/// process that died needs no cleaning: both files are still there and nobody holds the lock.
///
/// Neither file is followed if it is a link, and both must be plain files. A FIFO at either would
/// wait for a writer, a link to a device could read without end, and a folder is not a lock.
pub(super) fn take(dir: &Path) -> Result<File, Error> {
    let path = dir.join("lock");
    // The file has no content. It exists to be locked.
    let file = open(&path, true).map_err(|e| refused(&path, &e))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(held(dir)),
        Err(TryLockError::Error(e)) => return Err(storage("lock", &path, e)),
    }
    // Before the write, so a lock.pid that is not a file is refused and left as it is. A regular
    // one, left by a process that ended without letting go, is written over.
    if let Err(e) = read_pid(dir) {
        return Err(refused(&dir.join("lock.pid"), &e));
    }
    // After the lock, so the id in the file is the holder's and a refused process never changes it.
    write_atomic(
        dir,
        "lock.pid",
        format!("{}\n", std::process::id()).as_bytes(),
    )?;
    Ok(file)
}

/// Opens the lock file without following a link: a link there is an error that [`is_link_error`]
/// knows. With `taking` it is opened for reading and writing and made if it is missing, as the
/// process that takes the lock does. Without it, as `sluice doctor` does, it is only read, and it
/// is still not followed, since doctor tries the lock on whatever it finds.
///
/// `O_NONBLOCK` is what makes a FIFO answer at once instead of waiting for a writer; the file is
/// then refused because it is not a regular file.
pub(super) fn open(path: &Path, taking: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(taking)
        .create(taking)
        .truncate(false);
    refuse_links(&mut options);
    let file = options.open(path).map_err(link_or_other)?;
    checked(file)
}

/// That an open file is a plain file and not a link. On Windows the open took the link itself, if
/// it is one, and the handle says so; on Unix `O_NOFOLLOW` refused it before the open.
fn checked(file: File) -> io::Result<File> {
    let meta = file.metadata()?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, LinkError));
    }
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, NotAFile));
    }
    Ok(file)
}

/// The process id in `dir/lock.pid`, or `None` when there is no such file or it does not hold a
/// number. At most 32 bytes are read, and the file is opened without a link and without waiting,
/// so a FIFO or a link to a device cannot hang a refused open or `sluice doctor`.
pub(super) fn read_pid(dir: &Path) -> io::Result<Option<u32>> {
    let path = dir.join("lock.pid");
    let mut options = OpenOptions::new();
    options.read(true);
    refuse_links(&mut options);
    let mut file = match options.open(&path) {
        Ok(file) => checked(file)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(link_or_other(e)),
    };
    let mut bytes = [0u8; 32];
    let mut filled = 0;
    while filled < bytes.len() {
        match file.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let text = std::str::from_utf8(&bytes[..filled]).unwrap_or_default();
    Ok(text.trim().parse().ok())
}

/// `O_NOFOLLOW` refuses a symbolic link with `ELOOP` and does nothing else, and `O_NONBLOCK` makes
/// a FIFO open at once.
#[cfg(unix)]
fn refuse_links(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

/// `FILE_FLAG_OPEN_REPARSE_POINT`: a link is opened itself and not followed, and [`open`] looks at
/// what it got. `FILE_FLAG_BACKUP_SEMANTICS` is what lets a folder open at all, so a folder or a
/// junction at the name is refused as what it is instead of answering "access denied".
#[cfg(windows)]
fn refuse_links(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
}

#[cfg(not(any(unix, windows)))]
fn refuse_links(_options: &mut OpenOptions) {}

/// The error of an open that refused a link, as [`LinkError`], and any other error as it was.
fn link_or_other(error: io::Error) -> io::Error {
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::EMLINK)) {
        return io::Error::new(io::ErrorKind::InvalidInput, LinkError);
    }
    error
}

/// What [`open`] says when the lock is a link.
#[derive(Debug)]
struct LinkError;

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("is a link and not a file of the home: remove it")
    }
}

impl std::error::Error for LinkError {}

/// What [`open`] says when the lock is a FIFO, a folder or a device.
#[derive(Debug)]
struct NotAFile;

impl fmt::Display for NotAFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("is not a file of the home: remove it")
    }
}

impl std::error::Error for NotAFile {}

/// Whether `error` came from [`open`] finding a link.
pub(super) fn is_link_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<LinkError>().is_some())
}

/// Whether `error` came from [`open`] finding something that is not a file.
pub(super) fn is_not_a_file(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<NotAFile>().is_some())
}

/// The policy error for a lock file or a pid file that is a link or not a file, and a storage
/// error for anything else.
fn refused(path: &Path, error: &io::Error) -> Error {
    if is_link_error(error) || is_not_a_file(error) {
        return Error::Policy(format!("{} {error}", path.display()));
    }
    storage(
        "open",
        path,
        io::Error::new(error.kind(), error.to_string()),
    )
}

/// The error for a lock another process holds, naming that process when its id can be read. The id
/// is that of whoever held the lock last, so in the instant between a process taking the lock and
/// writing its id, it can be the one before. That makes the name a hint and never a check.
fn held(dir: &Path) -> Error {
    let pid = read_pid(dir).ok().flatten();
    let who = match pid {
        Some(pid) => format!("another sluice process (pid {pid})"),
        None => "another sluice process".to_string(),
    };
    Error::Policy(format!(
        "{who} is using the store at {}; send both in one command instead",
        dir.display()
    ))
}

#[cfg(test)]
mod tests;
