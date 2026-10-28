//! Putting a finished file at its name without replacing a path nobody allowed, and the one
//! Windows fallback that clears a read-only bit to make that rename possible.

use std::{fs, io, path::Path};

use super::{clash, storage};
use crate::Error;

/// Moves `from` to `target`. With `force` an existing target is replaced; without it the rename
/// itself refuses one, so a file or link that appeared after the clash check is never overwritten.
/// A file system whose rename cannot refuse is tried with a hard link, which also refuses, and
/// then the transfer stops and says `--force` is the way to write it with a plain rename.
pub fn install(from: &Path, target: &Path, force: bool) -> Result<(), Error> {
    if force {
        return replace(from, target).map_err(|e| storage("install", target, e));
    }
    match fs::symlink_metadata(target) {
        Ok(_) => return Err(clash(target)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(storage("check", target, e)),
    }
    match rename_no_replace(from, target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(clash(target)),
        Err(e) if unsupported(&e) => link_no_replace(from, target),
        Err(e) => Err(storage("install", target, e)),
    }
}

/// The fallback for a rename that cannot refuse a name: a link fails when the name is taken, and
/// the unlink leaves the same one file behind. A file system that supports neither is refused
/// rather than checked and renamed, which would leave a moment for another process to write there.
pub(super) fn link_no_replace(from: &Path, target: &Path) -> Result<(), Error> {
    match fs::hard_link(from, target) {
        Ok(()) => {
            // The target is installed either way; a temporary name that stays only makes the
            // caller's folder removal do the unlink.
            let _ = fs::remove_file(from);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(clash(target)),
        Err(_) => Err(Error::Storage(format!(
            "cannot install {} without replacing a path that is already there: this file system \
             can neither rename without replacing nor link. Pass --force to write it with a plain \
             rename",
            target.display()
        ))),
    }
}

/// Errors from a rename that mean the flag itself is not supported, not that the call was refused.
/// NFS, FUSE, some macOS volumes and old kernels answer like this, and a link still works there.
/// `ENOSYS` and `EOPNOTSUPP` are 78 and 102 on macOS and 38 and 95 on Linux, and `ENOTSUP` is 45 on
/// macOS, so every platform's numbers for "this file system has no such flag" are here.
#[cfg(unix)]
pub(super) fn unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 38 | 45 | 78 | 95 | 102))
}

#[cfg(not(unix))]
pub(super) fn unsupported(_error: &io::Error) -> bool {
    false
}

/// Replaces `target` in one rename. Windows refuses a read-only destination; clearing that one bit
/// and trying the same rename again is the only fallback, and it loses no bytes.
///
/// The bit is cleared on the entry at `target` itself, through a handle that does not follow a link.
/// A link in the output must not let `--force` change a file outside it, which is what a path-based
/// call would do.
fn replace(from: &Path, target: &Path) -> io::Result<()> {
    match replace_once(from, target) {
        Ok(()) => Ok(()),
        Err(e) => match fs::symlink_metadata(target) {
            Ok(meta) if meta.permissions().readonly() => {
                clear_readonly(target)?;
                replace_once(from, target)
            }
            _ => Err(e),
        },
    }
}

/// Clears the read-only bit of the entry at `path`, and never of what a link points at.
#[cfg(unix)]
pub(super) fn clear_readonly(path: &Path) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags};

    // `O_NOFOLLOW` makes the open fail on a link, so the mode is only ever set on a file that is
    // already there. `fchmod` works through a handle, so the file the mode is set on is the file
    // that was opened.
    let handle = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let meta = rustix::fs::fstat(&handle)?;
    rustix::fs::fchmod(
        &handle,
        Mode::from_raw_mode((meta.st_mode & 0o7777) | 0o200),
    )
    .map_err(io::Error::from)
}

/// Clears the read-only attribute of the entry at `path`, and never of what a link points at: the
/// handle is opened with `OPEN_REPARSE_POINT`, which names the link itself.
#[cfg(windows)]
pub(super) fn clear_readonly(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let handle = fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)?;
    let mut permissions = handle.metadata()?.permissions();
    // This is Windows, where readonly is the only permission bit `set_permissions` holds.
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    handle.set_permissions(permissions)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn clear_readonly(path: &Path) -> io::Result<()> {
    let mut permissions = fs::symlink_metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
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

    // A device path names a device or a volume and not a file in a folder. Built into the verbatim
    // form below it becomes `\\?\UNC\.\...`, which names something else entirely, so it is refused
    // rather than turned into a path it is not.
    for path in [from, target] {
        if is_device_path(path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a device path is not a file path",
            ));
        }
    }

    /// The path as a verbatim wide string: `\\?\C:\...` or `\\?\UNC\server\share\...`, with `\`
    /// separators. `MoveFileExW` is limited to 260 characters without it, while `std::fs` works
    /// past that, so a deep output folder would otherwise fail only at the last step.
    fn verbatim(path: &Path) -> Vec<u16> {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let mut wide: Vec<u16> = absolute
            .as_os_str()
            .encode_wide()
            .map(|c| {
                if c == u16::from(b'/') {
                    u16::from(b'\\')
                } else {
                    c
                }
            })
            .collect();
        let slash = u16::from(b'\\');
        if wide.starts_with(&[slash, slash, u16::from(b'?'), slash]) {
            wide.push(0);
            return wide;
        }
        let unc = wide.starts_with(&[slash, slash]);
        let mut out: Vec<u16> = if unc {
            r"\\?\UNC\".encode_utf16().collect()
        } else {
            r"\\?\".encode_utf16().collect()
        };
        out.extend_from_slice(if unc { &wide[2..] } else { &wide[..] });
        out.push(0);
        out
    }
    let from = verbatim(from);
    let target = verbatim(target);
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

/// Whether `path` names a device rather than a file: a Windows device path starts with `\\.\`, as in
/// `\\.\C:` or `\\.\pipe\name`. Windows reads `/` as a separator too, so `//./pipe/name` names the
/// same device and is refused in the same way; the check normalises the separators before comparing,
/// which is the order the rename below uses when it turns the path into its verbatim form.
#[cfg(windows)]
pub(super) fn is_device_path(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    let mut wide = path.as_os_str().encode_wide().map(|c| {
        if c == u16::from(b'/') {
            u16::from(b'\\')
        } else {
            c
        }
    });
    let device = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'.'),
        u16::from(b'\\'),
    ];
    device.iter().all(|want| wide.next() == Some(*want))
}
