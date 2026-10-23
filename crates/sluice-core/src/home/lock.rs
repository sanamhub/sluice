//! The one-writer lock of a home.

use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
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
/// Neither file is followed if it is a link. Anyone who can write in the home could plant one, and
/// opening or writing through it would lock, truncate or overwrite a file outside the home. `lock`
/// that is a link is refused. `lock.pid` that is a link is replaced, because it is written through
/// a new file and a rename, which puts the new file where the link was and touches nothing beyond.
pub(super) fn take(dir: &Path) -> Result<File, Error> {
    let path = dir.join("lock");
    // The file has no content. It exists to be locked.
    let file = open(&path, true).map_err(|e| {
        if is_link_error(&e) {
            Error::Policy(format!("{} {LinkError}", path.display()))
        } else {
            storage("open", &path, e)
        }
    })?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(held(dir)),
        Err(TryLockError::Error(e)) => return Err(storage("lock", &path, e)),
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
pub(super) fn open(path: &Path, taking: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(taking)
        .create(taking)
        .truncate(false);
    refuse_links(&mut options);
    let file = options.open(path).map_err(link_or_other)?;
    // On Windows the open took the link itself, if it is one, and the handle says so.
    if file.metadata()?.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, LinkError));
    }
    Ok(file)
}

/// `O_NOFOLLOW`: opening a symbolic link fails with `ELOOP` and does nothing else.
#[cfg(unix)]
fn refuse_links(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

/// `FILE_FLAG_OPEN_REPARSE_POINT`: a link is opened itself and not followed, and [`open`] looks at
/// what it got.
#[cfg(windows)]
fn refuse_links(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
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
        f.write_str("is a link, and the lock must be a file of the home itself: remove it")
    }
}

impl std::error::Error for LinkError {}

/// Whether `error` came from [`open`] finding a link.
pub(super) fn is_link_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<LinkError>().is_some())
}

/// The error for a lock another process holds, naming that process when its id can be read. The id
/// is that of whoever held the lock last, so in the instant between a process taking the lock and
/// writing its id, it can be the one before. That makes the name a hint and never a check.
fn held(dir: &Path) -> Error {
    let pid = fs::read_to_string(dir.join("lock.pid"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    let who = match pid {
        Some(pid) => format!("another sluice process (pid {pid})"),
        None => "another sluice process".to_string(),
    };
    Error::Policy(format!("{who} is using the store at {}", dir.display()))
}
