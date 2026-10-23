//! One function per check. Each reads and returns a finding, and none writes.

use std::fs::{self, TryLockError};
use std::io;
use std::path::Path;

use super::super::{layout, setup};
use super::{Check, Finding, Location, Name};

/// The report when the home is not there: its finding, and every other check skipped.
pub(super) fn without_home(home: Finding) -> Vec<Check> {
    let mut checks = vec![Check::new(home)];
    for name in [
        Name::Access,
        Name::Layout,
        Name::Lock,
        Name::Key,
        Name::Space,
    ] {
        checks.push(Check::new(Finding::NeedsHome(name)));
    }
    checks
}

pub(super) fn home(location: Location) -> Finding {
    // The home itself may be a link, which `Home::open` follows once, so its own metadata follows
    // it. The folders inside are different: `open` refuses a link there, so doctor reads each with
    // `symlink_metadata` and reports the link instead of following it.
    match fs::metadata(&location.path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Finding::HomeMissing(location),
        Err(e) => Finding::HomeUnreadable(location, e.to_string()),
        Ok(meta) if !meta.is_dir() => Finding::HomeNotAFolder(location),
        Ok(_) => {
            let mut missing = Vec::new();
            for name in setup::FOLDERS {
                let folder = location.path.join(name);
                match fs::symlink_metadata(&folder) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Finding::HomeFolderIsLink(location, name);
                    }
                    Ok(meta) if meta.is_dir() => {}
                    Ok(_) => missing.push(name),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push(name),
                    Err(e) => return Finding::HomeUnreadable(location, e.to_string()),
                }
            }
            if missing.is_empty() {
                Finding::HomeReady(location)
            } else {
                Finding::HomeFoldersMissing(location, missing)
            }
        }
    }
}

/// `home` is the folder with its links followed, which is read, and `given` the path the user
/// gave, which a message names.
pub(super) fn layout(home: &Path, given: &Path) -> Finding {
    let read = layout::read_version(home).map_err(|e| super::super::as_given(e, home, given));
    classify_layout(read, layout::current())
}

/// The finding for what `VERSION` gave, against the version this build writes. Apart from the
/// read, so the tests can give a `current` above 1, which is the only version there is so far.
pub(super) fn classify_layout(read: Result<Option<u32>, crate::Error>, current: u32) -> Finding {
    match read {
        Err(e) => Finding::LayoutUnreadable(e.to_string()),
        Ok(None) => Finding::LayoutNoFile(current),
        Ok(Some(found)) if found == current => Finding::LayoutCurrent(found),
        Ok(Some(found)) if found < current => Finding::LayoutOlder { found, current },
        Ok(Some(found)) => Finding::LayoutNewer {
            found,
            known: current,
        },
    }
}

/// Tries the lock without waiting and lets it go at once, so a running sluice is never kept
/// waiting and the lock file is never created. A free lock with `lock.pid` still beside it was
/// left by a process that ended without letting go.
pub(super) fn lock(home: &Path, given: &Path) -> Finding {
    // Bounded and without following a link, so a FIFO or a link to a device cannot hang the report.
    let pid = match super::super::lock::read_pid(home) {
        Ok(pid) => pid,
        Err(e) => return Finding::LockUnreadable(named(given, "lock.pid", &e)),
    };
    let file = match super::super::lock::open(&home.join("lock"), false) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return free_or_stale(home, given, pid),
        Err(e) => return Finding::LockUnreadable(named(given, "lock", &e)),
    };
    match file.try_lock() {
        Ok(()) => {
            // Dropping the file would let go too. Unlocking first says so.
            let _ = file.unlock();
            free_or_stale(home, given, pid)
        }
        Err(TryLockError::WouldBlock) => Finding::LockHeld(pid),
        Err(TryLockError::Error(e)) => Finding::LockUnreadable(named(given, "lock", &e)),
    }
}

fn free_or_stale(home: &Path, given: &Path, pid: Option<u32>) -> Finding {
    match fs::symlink_metadata(home.join("lock.pid")) {
        Ok(meta) if meta.is_file() => Finding::LockStale(pid),
        Ok(_) => Finding::LockUnreadable(named(
            given,
            "lock.pid",
            &io::Error::new(io::ErrorKind::InvalidInput, NOT_A_FILE),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Finding::LockFree,
        Err(e) => Finding::LockUnreadable(named(given, "lock.pid", &e)),
    }
}

/// What a pid file that is not a file is, in the report. The same words as `lock`'s own refusal.
const NOT_A_FILE: &str = "is not a file of the home: remove it";

/// Who can reach the home folder. A check of its own, and not part of the key's, so a home and a
/// key that are both open show as two failures at once, and not one after the other is fixed.
///
/// `home` is the folder with its links followed, which is read, and `given` the path the user
/// gave, which an error names.
pub(super) fn access(home: &Path, given: &Path) -> Finding {
    home_not_private(home, given).unwrap_or(Finding::AccessPrivate)
}

/// The key's length and access, from its metadata and access list. Its bytes are never read.
pub(super) fn key(home: &Path, given: &Path) -> Finding {
    let path = home.join("identity.key");
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Finding::KeyMissing,
        Err(e) => return Finding::KeyUnreadable(named(given, "identity.key", &e)),
    };
    if let Some(open) = key_open_to_others(&path, given, &meta) {
        return open;
    }
    if meta.len() != 32 {
        return Finding::KeyWrongSize(meta.len());
    }
    Finding::KeyPrivate
}

/// An error of a file in the home, told with the path the user gave and not where its links lead.
fn named(given: &Path, file: &str, error: &io::Error) -> String {
    format!("{}: {error}", given.join(file).display())
}

/// On Unix, the effective user owns the home folder, and no access is left for group or other,
/// which the store creates with mode 0700, so that the key is not reachable through it even if its
/// own mode were loosened. The owner is looked at first: an owner can change the mode.
#[cfg(unix)]
fn home_not_private(home: &Path, given: &Path) -> Option<Finding> {
    home_not_private_for(home, given, rustix::process::geteuid().as_raw())
}

/// [`home_not_private`] for the user `me`, so a test can be someone a folder does not belong to.
#[cfg(unix)]
pub(super) fn home_not_private_for(home: &Path, given: &Path, me: u32) -> Option<Finding> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match fs::metadata(home) {
        Ok(meta) if meta.uid() != me => Some(Finding::AccessOwnedByOther),
        Ok(meta) if meta.permissions().mode() & 0o077 != 0 => Some(Finding::AccessOpenToOthers(
            meta.permissions().mode() & 0o7777,
        )),
        Ok(_) => None,
        Err(e) => Some(unreadable(given, e)),
    }
}

/// The error names the home folder, so the user looks there and not at `identity.key`.
fn unreadable(home: &Path, error: io::Error) -> Finding {
    Finding::AccessUnreadable(format!("{}: {error}", home.display()))
}

/// On Windows, the user or `Administrators` owns the home folder, and it has the private access
/// list the store gives a new home, read back and compared. The key has a list of its own, and the
/// folder's list still decides who can add, swap or delete files beside it. The owner is looked at
/// first: an owner can rewrite the list.
#[cfg(windows)]
fn home_not_private(home: &Path, given: &Path) -> Option<Finding> {
    use super::super::private_windows::{is_owned_by_us, is_private_dir};
    match is_owned_by_us(home) {
        Ok(true) => {}
        Ok(false) => return Some(Finding::AccessOwnedByOther),
        Err(e) => return Some(unreadable(given, e)),
    }
    match is_private_dir(home) {
        Ok(true) => None,
        Ok(false) => Some(Finding::AccessAclNotPrivate),
        Err(e) => Some(unreadable(given, e)),
    }
}

#[cfg(not(any(unix, windows)))]
fn home_not_private(_home: &Path, _given: &Path) -> Option<Finding> {
    None
}

/// The rule the store applies to the key on Unix: no access at all for group or other.
#[cfg(unix)]
fn key_open_to_others(_key: &Path, _given: &Path, meta: &fs::Metadata) -> Option<Finding> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o7777;
    (mode & 0o077 != 0).then_some(Finding::KeyOpenToOthers(mode))
}

/// The private access list the store gives a key on Windows, read back and compared.
#[cfg(windows)]
fn key_open_to_others(key: &Path, given: &Path, _meta: &fs::Metadata) -> Option<Finding> {
    match super::super::private_windows::is_private_file(key) {
        Ok(true) => None,
        Ok(false) => Some(Finding::KeyAclNotPrivate),
        Err(e) => Some(Finding::KeyUnreadable(named(given, "identity.key", &e))),
    }
}

#[cfg(not(any(unix, windows)))]
fn key_open_to_others(_key: &Path, _given: &Path, _meta: &fs::Metadata) -> Option<Finding> {
    None
}

pub(super) fn space(home: &Path) -> Finding {
    match free_bytes(home) {
        Ok(free) => Finding::SpaceFree(free),
        Err(e) => Finding::SpaceUnreadable(e.to_string()),
    }
}

/// Bytes free to this user, which is less than free on the volume when some are reserved for the
/// superuser.
#[cfg(unix)]
fn free_bytes(home: &Path) -> io::Result<u64> {
    let stat = rustix::fs::statvfs(home)?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(windows)]
fn free_bytes(home: &Path) -> io::Result<u64> {
    super::super::private_windows::free_bytes(home)
}

#[cfg(not(any(unix, windows)))]
fn free_bytes(_home: &Path) -> io::Result<u64> {
    Err(io::Error::other("this platform has no call for it"))
}
