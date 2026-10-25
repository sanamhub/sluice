//! The home's path with every link followed, and the fallback for a file system that cannot answer.
//!
//! On Windows the answer is kept in the verbatim form, `\\?\C:\...`, and every call that touches
//! the home takes it: without the prefix Win32 normalises a path again, so a component with a
//! trailing dot or space can name another folder, a component named `CON` can reach a device, and a
//! path over 260 characters fails.
//!
//! Some file systems cannot be asked at all, and the fallback walks the path instead. A link above
//! the ancestor that answered was followed by the call that answered, and is the one the caller
//! cannot see; a link below it is refused by name, since the file system that cannot say where a
//! path leads cannot check a link there either.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::Error;

use super::super::storage;

/// `path` with every link followed, as `std::fs::canonicalize` gives it. On Windows that is the
/// verbatim form, `\\?\C:\...`, and it stays that way here: every call that touches the home takes
/// it, and a message shows the path as the user gave it (see `as_given`).
///
/// Some file systems cannot be asked: a RAM disk, a WinFsp or Dokan drive, a VirtualBox share, an
/// old SMB server. When the system answers that the call is unsupported, and the home itself is
/// not a link, the path given is the one to use. Any other error is refused, with a verb that fits
/// it: access denied is not "cannot find".
pub(in crate::home) fn resolve(path: &Path) -> Result<PathBuf, Error> {
    resolve_with(path, |path| fs::canonicalize(path))
}

/// [`resolve`] with the call that follows links as a parameter, so a test can be a file system that
/// does not have it.
pub(in crate::home) fn resolve_with(
    path: &Path,
    canonicalize: fn(&Path) -> io::Result<PathBuf>,
) -> Result<PathBuf, Error> {
    match canonicalize(path) {
        Ok(root) => Ok(root),
        Err(e) if cannot_ask(&e) => match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => Err(cannot_resolve(path, &e)),
            Ok(_) => resolve_below_links(path, canonicalize),
            Err(other) => Err(cannot_resolve(path, &other)),
        },
        Err(e) => Err(cannot_resolve(path, &e)),
    }
}

/// The file system cannot resolve `path`, so walk up to the longest ancestor it can resolve, and
/// check every component from that one down to `path` itself. A link above the resolved ancestor
/// was followed by the call that resolved it, and is the one the caller cannot see; a link below it
/// is refused by name, since the file system that cannot answer cannot say where it leads either.
/// This is what makes a home under `/var` on macOS, or under FreeBSD's `/home`, open: the system
/// made those links.
fn resolve_below_links(
    path: &Path,
    canonicalize: fn(&Path) -> io::Result<PathBuf>,
) -> Result<PathBuf, Error> {
    let mut child = path;
    while let Some(ancestor) = child.parent().filter(|p| !p.as_os_str().is_empty()) {
        match canonicalize(ancestor) {
            Ok(resolved) => return resolved_and_checked(resolved, ancestor, path),
            Err(e) if cannot_ask(&e) => child = ancestor,
            Err(e) => return Err(cannot_resolve(path, &e)),
        }
    }
    absolute_without_links(path)
}

/// `resolved`, the resolved `ancestor`, with every component from `ancestor` down to `path` pushed
/// onto it, each checked with `symlink_metadata` for a link. Those are the components the system
/// cannot resolve, so each is refused by the path the user gave. The tail comes from `path` and not
/// from the ancestor the walk stopped at: two levels below the resolved one are as much part of the
/// home as one, and a tail dropped here would put the store's folders in another folder.
fn resolved_and_checked(resolved: PathBuf, ancestor: &Path, path: &Path) -> Result<PathBuf, Error> {
    let mut out = resolved;
    let mut shown = ancestor.to_path_buf();
    for component in path.strip_prefix(ancestor).unwrap_or(path).components() {
        out.push(component);
        shown.push(component);
        match fs::symlink_metadata(&out) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(link_in_the_way(&shown)),
            Ok(_) => {}
            Err(e) => return Err(storage("read", &out, e)),
        }
    }
    Ok(out)
}

/// The path given, made absolute, after every ancestor is checked with `symlink_metadata` and any
/// link is refused by name. A file system that cannot resolve a path cannot say where it leads, so
/// the one thing left is to walk it. The home itself was checked by the caller; a link higher up
/// would still put everything under wherever it points.
fn absolute_without_links(path: &Path) -> Result<PathBuf, Error> {
    let absolute = std::path::absolute(path).map_err(|e| storage("resolve", path, e))?;
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(link_in_the_way(ancestor));
            }
            Ok(_) => {}
            Err(e) => return Err(storage("read", ancestor, e)),
        }
    }
    Ok(absolute)
}

/// The refusal for a link the file system that cannot resolve the home cannot see through, by the
/// path the user gave.
fn link_in_the_way(path: &Path) -> Error {
    Error::Policy(format!(
        "{} is a link, and this file system cannot say where a path leads, so sluice cannot check \
         where the home is: remove the link, or give the home's real path",
        path.display()
    ))
}

/// The file system does not implement the call that follows links, as opposed to refusing it.
fn cannot_ask(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Unsupported || invalid_function(error)
}

/// `ERROR_INVALID_FUNCTION`, which a Windows file system answers when it has no such call. Only on
/// Windows, where the number means that: on Unix 1 is `EPERM`.
#[cfg(windows)]
fn invalid_function(error: &io::Error) -> bool {
    use windows_sys::Win32::Foundation::ERROR_INVALID_FUNCTION;
    error.raw_os_error() == Some(ERROR_INVALID_FUNCTION as i32)
}

#[cfg(not(windows))]
fn invalid_function(_error: &io::Error) -> bool {
    false
}

/// The refusal for a home that cannot be resolved, by the path the user gave.
pub(in crate::home) fn cannot_resolve(path: &Path, error: &io::Error) -> Error {
    let verb = match error.kind() {
        io::ErrorKind::NotFound => "find",
        io::ErrorKind::PermissionDenied => "get access to",
        _ => "resolve",
    };
    storage(verb, path, io::Error::new(error.kind(), error.to_string()))
}
