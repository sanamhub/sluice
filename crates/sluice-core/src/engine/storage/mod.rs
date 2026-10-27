//! The disk side of one receive: the space it needs, where its partial store lives, a private
//! temporary folder, and installing the finished file without replacing a path nobody allowed.

mod meta;

pub use meta::MetaStore;

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

/// Refuses when `dir` does not have room for a transfer of `content` bytes plus the store beside
/// it. The store is what a receive writes before it writes the file, so this is the volume that
/// has to hold both during the transfer.
pub fn ensure_space(dir: &Path, content: u64) -> Result<(), Error> {
    let available = available_bytes(dir)?;
    let needed = needed_bytes(content);
    if available < needed {
        return Err(Error::Storage(format!(
            "{} has {available} bytes free and this transfer needs about {needed}",
            dir.display()
        )));
    }
    Ok(())
}

/// The name of the file inside a partial store that says which ticket made it and which top-level
/// entries it made room for. Without it a store left by a failed run cannot be told from one
/// another ticket made under a name that happens to match, and a resume would trust bytes nobody
/// checked.
pub const MARKER: &str = "ticket";

/// What a partial store's marker says: the transfer that made the folder, and the top-level names
/// that transfer created in the output. A resume may write into those names and no others.
pub struct Owner {
    pub root: String,
    pub names: std::collections::BTreeSet<String>,
}

/// Reads the marker of `dir` without changing anything. A folder with no marker, or one this user
/// cannot read, is not a resume; [`claim`] deals with it once every check has passed.
pub fn recorded(dir: &Path) -> Result<Option<Owner>, Error> {
    let marker = dir.join(MARKER);
    let text = match fs::read_to_string(&marker) {
        Ok(text) => text,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(storage("read", &marker, e)),
    };
    Ok(parse_marker(&text))
}

/// The first line is the root; every `name ` line after it is a top-level entry of the transfer.
fn parse_marker(text: &str) -> Option<Owner> {
    let mut lines = text.lines();
    let root = lines.next()?.trim();
    if root.is_empty() {
        return None;
    }
    let names = lines
        .filter_map(|line| line.strip_prefix("name "))
        .map(str::to_string)
        .collect();
    Some(Owner {
        root: root.to_string(),
        names,
    })
}

/// Makes `dir` an empty store of this receive, removing a folder that is not one. Called only after
/// every check has passed, so a refusal leaves no store for a later run to resume.
pub fn claim(dir: &Path) -> Result<(), Error> {
    remove_quietly(dir);
    fs::create_dir_all(dir).map_err(|e| storage("create", dir, e))
}

/// Writes the marker of a store this receive just made, naming the top-level entries it will
/// create. A later run reads it to tell this receive's folders from the user's.
pub fn remember(
    dir: &Path,
    root: &str,
    names: &std::collections::BTreeSet<String>,
) -> Result<(), Error> {
    let marker = dir.join(MARKER);
    let mut text = format!("{root}\n");
    for name in names {
        text.push_str("name ");
        text.push_str(name);
        text.push('\n');
    }
    fs::write(&marker, text).map_err(|e| storage("write", &marker, e))
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
fn link_no_replace(from: &Path, target: &Path) -> Result<(), Error> {
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
mod tests;
