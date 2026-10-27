//! The disk side of one receive: the space it needs, where its partial store lives, a private
//! temporary folder, and installing the finished file without replacing a path nobody allowed.

use std::{
    fs::{self},
    io,
    path::{Path, PathBuf},
};

use crate::Error;

/// How much room a receive of `content` bytes needs before it starts. The store keeps the partial
/// blob and its outboard, about 0.4% of the content, and the manifest and metadata are small. The
/// finished file is moved out of the store, not copied, so this is a peak, not 2x.
pub fn needed_bytes(content: u64) -> u64 {
    const OUTBOARD_PARTS_PER_THOUSAND: u64 = 8;
    const SLACK: u64 = 1 << 20;
    content
        .saturating_add(content / 1000 * OUTBOARD_PARTS_PER_THOUSAND)
        .saturating_add(SLACK)
}

/// Bytes free to this user on the volume that holds `path`.
pub fn available_bytes(path: &Path) -> Result<u64, Error> {
    imp::available(path).map_err(|e| {
        Error::Storage(format!(
            "cannot read the free space at {}: {e}",
            path.display()
        ))
    })
}

/// The hidden store beside the destination. A per-transfer store keeps partial data on the same
/// filesystem as the file it will become, so a finished blob can be moved into place.
pub fn partial_dir(out: &Path, root: &str) -> PathBuf {
    let short: String = root.chars().take(16).collect();
    out.join(format!(".sluice-partial-{short}"))
}

/// A temporary folder for this write, made with one `create_dir` so it cannot follow a link that
/// someone left under a fixed name. The caller removes it when the write ends.
pub fn temp_dir(out: &Path) -> Result<PathBuf, Error> {
    let mut last = None;
    for _ in 0..8 {
        let mut bytes = [0u8; 8];
        if let Err(e) = getrandom::fill(&mut bytes) {
            return Err(Error::Storage(format!(
                "cannot get a random name for a temporary folder: {e}"
            )));
        }
        let path = out.join(format!(".sluice-tmp-{:016x}", u64::from_le_bytes(bytes)));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(storage("create", &path, e)),
        }
    }
    Err(storage(
        "create a temporary folder in",
        out,
        last.unwrap_or_else(|| io::Error::other("no name was free")),
    ))
}

/// A name inside the temporary folder that no entry can have, because the folder is new and only
/// this writer names anything in it.
pub fn temp_path(dir: &Path, index: u64) -> PathBuf {
    dir.join(format!("{index}"))
}

/// Moves `from` to `target`. With `force` an existing target is replaced; without it the rename
/// itself refuses one, so a file or link that appeared after the clash check is never overwritten.
pub fn install(from: &Path, target: &Path, force: bool) -> Result<(), Error> {
    let done = if force {
        replace(from, target)
    } else {
        match fs::symlink_metadata(target) {
            Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => rename_no_replace(from, target),
            Err(e) => Err(e),
        }
    };
    match done {
        Ok(()) => Ok(()),
        Err(e) if !force && e.kind() == io::ErrorKind::AlreadyExists => Err(clash(target)),
        Err(e) => Err(storage("install", target, e)),
    }
}

/// Replaces `target` in one rename. Windows refuses a read-only destination; clearing that one bit
/// and trying the same rename again is the only fallback, and it loses no bytes.
fn replace(from: &Path, target: &Path) -> io::Result<()> {
    match replace_once(from, target) {
        Ok(()) => Ok(()),
        Err(e) if target.exists() => {
            let meta = fs::symlink_metadata(target)?;
            if !meta.permissions().readonly() {
                return Err(e);
            }
            let mut permissions = meta.permissions();
            // This is Windows, where readonly is the only permission bit `set_permissions` holds.
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(target, permissions)?;
            replace_once(from, target)
        }
        Err(e) => Err(e),
    }
}

#[cfg(windows)]
fn replace_once(from: &Path, target: &Path) -> io::Result<()> {
    move_file(from, target, true)
}

#[cfg(not(windows))]
fn replace_once(from: &Path, target: &Path) -> io::Result<()> {
    fs::rename(from, target)
}

/// A rename that fails rather than replace a name that is already there.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_no_replace(from: &Path, target: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        target,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(windows)]
fn rename_no_replace(from: &Path, target: &Path) -> io::Result<()> {
    move_file(from, target, false)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace(from: &Path, target: &Path) -> io::Result<()> {
    // A hard link fails when the name exists, and the unlink leaves the same single file behind.
    fs::hard_link(from, target)?;
    fs::remove_file(from)
}

#[cfg(windows)]
// SAFETY: the call needs two NUL-terminated wide strings, which outlive it.
#[allow(unsafe_code)]
fn move_file(from: &Path, target: &Path, replace: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let from = wide(from);
    let target = wide(target);
    let flags = if replace {
        MOVEFILE_REPLACE_EXISTING
    } else {
        0
    };
    // SAFETY: both strings are NUL-terminated and live for the call.
    if unsafe { MoveFileExW(from.as_ptr(), target.as_ptr(), flags) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The one clash message, naming the first path and both ways out.
pub fn clash(target: &Path) -> Error {
    Error::Policy(format!(
        "{} already exists; pass --force to replace it, or choose another --out",
        target.display()
    ))
}

pub fn storage(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Storage(format!("cannot {doing} {}: {source}", path.display()))
}

pub fn policy_path(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Policy(format!("cannot {doing} {}: {source}", path.display()))
}

/// Removes a file or folder that this receive made, ignoring a failure: the transfer itself
/// succeeded or failed for a reason worth reporting first.
pub fn remove_quietly(path: &Path) {
    let _ = fs::remove_dir_all(path);
    let _ = fs::remove_file(path);
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    pub(super) fn available(path: &Path) -> Result<u64, std::io::Error> {
        let stat = rustix::fs::statvfs(path)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    pub(super) fn available(path: &Path) -> Result<u64, std::io::Error> {
        crate::home::private_windows::free_bytes(path)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    pub(super) fn available(_path: &Path) -> Result<u64, std::io::Error> {
        Err(std::io::Error::other("this platform has no call for it"))
    }
}

#[cfg(test)]
mod tests {
    use super::{install, needed_bytes, partial_dir, temp_dir};
    use std::path::Path;

    #[test]
    fn a_hundred_gigabytes_needs_about_a_hundred_and_not_two_hundred() {
        let need = needed_bytes(100 << 30);
        assert!(need > 100 << 30);
        assert!(need < 101 << 30, "{need}");
    }

    #[test]
    fn the_partial_store_is_hidden_beside_the_output() {
        let path = partial_dir(Path::new("/out"), "0123456789abcdef0123");
        assert_eq!(path, Path::new("/out/.sluice-partial-0123456789abcdef"));
    }

    #[test]
    fn each_temporary_folder_has_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let first = temp_dir(dir.path()).unwrap();
        let second = temp_dir(dir.path()).unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
    }

    #[test]
    fn an_install_without_force_leaves_the_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let incoming = dir.path().join("incoming");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&incoming, b"new").unwrap();
        let err = install(&incoming, &target, false).unwrap_err();
        assert!(matches!(err, crate::Error::Policy(_)), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(incoming.exists());
    }

    #[test]
    fn a_file_that_appears_after_the_check_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("late");
        let incoming = dir.path().join("incoming");
        std::fs::write(&incoming, b"new").unwrap();
        // The plan ran while the target was absent; another process made it since.
        std::fs::write(&target, b"theirs").unwrap();
        assert!(install(&incoming, &target, false).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
    }

    #[test]
    fn an_install_with_force_replaces_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let incoming = dir.path().join("incoming");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&incoming, b"new").unwrap();
        install(&incoming, &target, true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!incoming.exists());
    }
}
