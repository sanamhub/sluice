//! Who owns an entry of a home: the answer that decides whether a folder, a key or a lock is this
//! user's. Root and `SYSTEM` are accepted with this user, since they can already do anything to the
//! files, so a home a backup tool restored still opens.

use std::io;
use std::path::Path;

/// Whether this user made the entry at `path`: on Unix its uid is the effective uid or root, and on
/// Windows its owner is this user, `Administrators` or `SYSTEM`. The owner is read from a handle
/// opened without following a link and without waiting, so it is the owner of the object the name
/// holds now. A folder this user may not open, as another user's mode 0700 folder is, still has an
/// owner that `lstat` reads without the access the open needed.
#[cfg(unix)]
pub(in crate::home) fn owned_by_this_user(path: &Path) -> io::Result<bool> {
    use std::{fs, os::unix::fs::MetadataExt};

    use rustix::fs::{Mode, OFlags};

    // This name may hold a file and not a folder. A FIFO here would wait for a writer, and the
    // caller refuses it by kind, so the open does not block and that refusal stays reachable.
    let flags = OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(file) => Ok(accepted_uid(rustix::fs::fstat(&file)?.st_uid)),
        Err(e) if e == rustix::io::Errno::ACCESS || e == rustix::io::Errno::PERM => {
            // The folder cannot be opened, and the owner is what decides whether that refusal is
            // "another user's" or a broken file system. lstat needs no access to the entry.
            Ok(accepted_uid(fs::symlink_metadata(path)?.uid()))
        }
        Err(e) => Err(e.into()),
    }
}

/// Root can already do anything to every file, so a folder it owns is as good as this user's, and
/// a home a backup tool restored as root still opens.
#[cfg(unix)]
pub(super) fn accepted_uid(uid: u32) -> bool {
    uid == rustix::process::geteuid().as_raw() || uid == 0
}

#[cfg(windows)]
pub(in crate::home) fn owned_by_this_user(path: &Path) -> io::Result<bool> {
    crate::home::private_windows::is_owned_by_this_user(path)
}

#[cfg(not(any(unix, windows)))]
pub(in crate::home) fn owned_by_this_user(_path: &Path) -> io::Result<bool> {
    Ok(true)
}
