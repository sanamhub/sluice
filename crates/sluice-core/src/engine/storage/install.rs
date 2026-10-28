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
#[cfg(unix)]
fn unsupported(error: &io::Error) -> bool {
    // EINVAL, ENOSYS, ENOTSUP and EOPNOTSUPP. The last two share a number on Linux.
    matches!(error.raw_os_error(), Some(22 | 38 | 45 | 95))
}

#[cfg(not(unix))]
fn unsupported(_error: &io::Error) -> bool {
    false
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
