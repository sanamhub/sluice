//! One function per check. Each reads and returns a finding, and none writes.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::Path;

use super::super::layout;
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
    // `symlink_metadata` would call a link to a folder a non-folder, and the store follows it.
    match fs::metadata(&location.path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Finding::HomeMissing(location),
        Err(e) => Finding::HomeUnreadable(location, e.to_string()),
        Ok(meta) if !meta.is_dir() => Finding::HomeNotAFolder(location),
        Ok(_) => {
            let missing: Vec<&'static str> = layout::FOLDERS
                .into_iter()
                .filter(|name| !location.path.join(name).is_dir())
                .collect();
            if missing.is_empty() {
                Finding::HomeReady(location)
            } else {
                Finding::HomeFoldersMissing(location, missing)
            }
        }
    }
}

pub(super) fn layout(home: &Path) -> Finding {
    classify_layout(layout::read_version(home), layout::current())
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
pub(super) fn lock(home: &Path) -> Finding {
    let pid = read_pid(home);
    let file = match File::open(home.join("lock")) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return free_or_stale(home, pid),
        Err(e) => return Finding::LockUnreadable(e.to_string()),
    };
    match file.try_lock() {
        Ok(()) => {
            // Dropping the file would let go too. Unlocking first says so.
            let _ = file.unlock();
            free_or_stale(home, pid)
        }
        Err(TryLockError::WouldBlock) => Finding::LockHeld(pid),
        Err(TryLockError::Error(e)) => Finding::LockUnreadable(e.to_string()),
    }
}

fn free_or_stale(home: &Path, pid: Option<u32>) -> Finding {
    if home.join("lock.pid").exists() {
        Finding::LockStale(pid)
    } else {
        Finding::LockFree
    }
}

fn read_pid(home: &Path) -> Option<u32> {
    fs::read_to_string(home.join("lock.pid"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// Who can reach the home folder. A check of its own, and not part of the key's, so a home and a
/// key that are both open show as two failures at once, and not one after the other is fixed.
pub(super) fn access(home: &Path) -> Finding {
    home_open_to_others(home).unwrap_or(Finding::AccessPrivate)
}

/// The key's length and access, from its metadata and access list. Its bytes are never read.
pub(super) fn key(home: &Path) -> Finding {
    let path = home.join("identity.key");
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Finding::KeyMissing,
        Err(e) => return Finding::KeyUnreadable(e.to_string()),
    };
    if let Some(open) = key_open_to_others(&path, &meta) {
        return open;
    }
    if meta.len() != 32 {
        return Finding::KeyWrongSize(meta.len());
    }
    Finding::KeyPrivate
}

/// On Unix, no access for group or other to the home folder, which the store creates with mode
/// 0700, so that the key is not reachable through it even if its own mode were loosened.
#[cfg(unix)]
fn home_open_to_others(home: &Path) -> Option<Finding> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(home) {
        Ok(meta) if meta.permissions().mode() & 0o077 != 0 => Some(Finding::HomeOpenToOthers(
            meta.permissions().mode() & 0o7777,
        )),
        Ok(_) => None,
        Err(e) => Some(unreadable(home, e)),
    }
}

/// The error names the home folder, so the user looks there and not at `identity.key`.
fn unreadable(home: &Path, error: io::Error) -> Finding {
    Finding::AccessUnreadable(format!("{}: {error}", home.display()))
}

/// On Windows, the private access list the store gives a new home, read back and compared. The
/// key has a list of its own, and the folder's list still decides who can add, swap or delete
/// files beside it.
#[cfg(windows)]
fn home_open_to_others(home: &Path) -> Option<Finding> {
    match super::super::private_windows::is_private_dir(home) {
        Ok(true) => None,
        Ok(false) => Some(Finding::HomeAclNotPrivate),
        Err(e) => Some(unreadable(home, e)),
    }
}

#[cfg(not(any(unix, windows)))]
fn home_open_to_others(_home: &Path) -> Option<Finding> {
    None
}

/// The rule the store applies to the key on Unix: no access at all for group or other.
#[cfg(unix)]
fn key_open_to_others(_key: &Path, meta: &fs::Metadata) -> Option<Finding> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o7777;
    (mode & 0o077 != 0).then_some(Finding::KeyOpenToOthers(mode))
}

/// The private access list the store gives a key on Windows, read back and compared.
#[cfg(windows)]
fn key_open_to_others(key: &Path, _meta: &fs::Metadata) -> Option<Finding> {
    match super::super::private_windows::is_private_file(key) {
        Ok(true) => None,
        Ok(false) => Some(Finding::KeyAclNotPrivate),
        Err(e) => Some(Finding::KeyUnreadable(e.to_string())),
    }
}

#[cfg(not(any(unix, windows)))]
fn key_open_to_others(_key: &Path, _meta: &fs::Metadata) -> Option<Finding> {
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
