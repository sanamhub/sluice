//! What the identity key must be, and what is refused: a link, another kind, a second name, another
//! owner, and on Unix a mode other users can read. Each refusal names the file, and the owner's
//! message names the platform's own command for taking it back.
//!
//! The checks read the handle the key was opened through, so the file checked is the file that is
//! read. [`check_unopened`] is the same four refusals for a key this user may not read, where no
//! handle can be had and the name is what is left.

use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::Path;

use crate::Error;

use super::super::storage;

/// The key's owner, its kind and its link count, read from the handle it is read through. A name
/// is not proof of what it holds: the key is opened once, without following a link, and every
/// check and the read after them are of that one object.
pub(super) fn check_key(
    file: &File,
    path: &Path,
    owner: fn(&File) -> io::Result<bool>,
) -> Result<(), Error> {
    let meta = file.metadata().map_err(|e| storage("inspect", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(is_a_link(path)));
    }
    if !meta.is_file() {
        return Err(Error::Policy(not_a_regular_file(path)));
    }
    let links = links(file, &meta).map_err(|e| storage("inspect", path, e))?;
    if links != 1 {
        return Err(Error::Policy(too_many_links(path, links)));
    }
    match owner(file) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Error::Policy(owned_by_other(path))),
        Err(e) => Err(storage("read the owner of", path, e)),
    }
}

/// The entry at `path` checked by name, for a key this user may not read: the same four refusals
/// [`check_key`] makes, in the same order and the same words. `symlink_metadata` reads the kind and
/// the count without access to the file. The owner is the check `Home::open` runs on a folder, by
/// path, because the handle `check_key` needs is the one that was refused; on Windows that reads it
/// through a handle that asks only for `READ_CONTROL`, and the count through the same handle.
pub(super) fn check_unopened(path: &Path) -> Result<(), Error> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        // Gone between the open and this look: there is nothing to refuse, and the new key takes
        // the name.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(storage("inspect", path, e)),
    };
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(is_a_link(path)));
    }
    if !meta.is_file() {
        return Err(Error::Policy(not_a_regular_file(path)));
    }
    let links = links_at(path, &meta).map_err(|e| storage("inspect", path, e))?;
    if links != 1 {
        return Err(Error::Policy(too_many_links(path, links)));
    }
    match super::super::setup::owned_by_this_user(path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Error::Policy(owned_by_other(path))),
        Err(e) => Err(storage("read the owner of", path, e)),
    }
}

/// Refuses a key that group or other can read, write or run. The mode comes from the handle that
/// the key will be read through, so the file checked is the file read, even if the path is
/// swapped in between. It only reads: loosening it was somebody's choice, and a file that sluice
/// changed would hide that the key may have been exposed. `rotate` never reads the old key, so it
/// is the way out that does not need the file fixed first.
#[cfg(unix)]
pub(super) fn refuse_readable_by_others(file: &File, path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file
        .metadata()
        .map_err(|e| storage("inspect", path, e))?
        .permissions()
        .mode()
        & 0o7777;
    if mode & 0o077 == 0 {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "{} has mode {mode:04o}, so other users may be able to read the identity key. \
         Run `chmod 600 {}` and try again",
        path.display(),
        path.display()
    )))
}

/// Windows has no mode bits. A new key gets its own access list when it is made, and one that
/// was loosened afterwards is not looked for, because that would need a security call on every
/// load.
#[cfg(not(unix))]
pub(super) fn refuse_readable_by_others(_file: &File, _path: &Path) -> Result<(), Error> {
    Ok(())
}

/// The message for a key another user owns, which says how to take it back. Doctor says the same
/// thing about the same key.
fn owned_by_other(path: &Path) -> String {
    format!(
        "{} belongs to another user, who could change it whatever sluice sets. {}, or move it \
         elsewhere and run `sluice id` for a new identity",
        path.display(),
        take_back(path)
    )
}

/// The platform's own commands for taking a file back from another user.
#[cfg(unix)]
fn take_back(path: &Path) -> String {
    format!(
        "Run `sudo chown $(id -un) {}` to take it back",
        path.display()
    )
}

/// The platform's own commands for taking a file back from another user.
#[cfg(windows)]
fn take_back(path: &Path) -> String {
    let shown = path.display();
    format!(
        "Run `takeown /f {shown}` and `icacls {shown} /inheritance:r /grant:r %USERNAME%:F` to \
         take it back"
    )
}

#[cfg(not(any(unix, windows)))]
fn take_back(_path: &Path) -> String {
    "Take ownership of it back".to_string()
}

pub(super) fn is_a_link(path: &Path) -> String {
    format!(
        "{} is a link and not the identity key: remove it, and run `sluice id` for a new identity",
        path.display()
    )
}

fn not_a_regular_file(path: &Path) -> String {
    format!(
        "{} is not a regular file, and the identity key must be one: move it elsewhere, and run \
         `sluice id` for a new identity",
        path.display()
    )
}

fn too_many_links(path: &Path, links: u64) -> String {
    format!(
        "{} has {links} links, and the identity key must have one: remove the other names, or \
         move it elsewhere and run `sluice id` for a new identity",
        path.display()
    )
}

/// The number of names the key has, from the handle it was opened through.
#[cfg(unix)]
fn links(_file: &File, meta: &fs::Metadata) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.nlink())
}

#[cfg(windows)]
fn links(file: &File, _meta: &fs::Metadata) -> io::Result<u64> {
    super::super::private_windows::links_of(file)
}

#[cfg(not(any(unix, windows)))]
fn links(_file: &File, _meta: &fs::Metadata) -> io::Result<u64> {
    Ok(1)
}

/// The number of names the entry at `path` has, for a caller with no handle to it.
#[cfg(unix)]
fn links_at(_path: &Path, meta: &fs::Metadata) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.nlink())
}

#[cfg(windows)]
fn links_at(path: &Path, _meta: &fs::Metadata) -> io::Result<u64> {
    super::super::private_windows::links_of_path(path)
}

#[cfg(not(any(unix, windows)))]
fn links_at(_path: &Path, _meta: &fs::Metadata) -> io::Result<u64> {
    Ok(1)
}
