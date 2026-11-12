//! The walk on Unix: `openat2` on Linux, which refuses a symbolic link, a magic link, a mount point
//! and a way out of the folder in the kernel, and `openat` with `O_NOFOLLOW` where the kernel has no
//! `openat2` and on every other Unix, with a check that each component is on the root's volume.

use std::fs::File;
use std::os::fd::AsFd;
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::fs::{AtFlags, FileType, Mode, OFlags, mkdirat, openat, statat};
use rustix::io::Errno;
use sluice_core::engine::{facts, volume_of};

use super::{NOT_PLAIN, Opened, Start, Use};
use crate::error::ApiError;

/// Set once the kernel has said it has no `openat2`, so it is asked once.
#[cfg(any(target_os = "linux", target_os = "android"))]
static NO_OPENAT2: AtomicBool = AtomicBool::new(false);

enum Failed {
    Missing,
    Refused,
    Other(Errno),
}

fn classify(errno: Errno) -> Failed {
    match errno {
        Errno::NOENT => Failed::Missing,
        // A link under `O_NOFOLLOW`, a file where a folder is needed, a mount point under
        // `RESOLVE_NO_XDEV` and a way out under `RESOLVE_BENEATH` all end here.
        Errno::LOOP | Errno::NOTDIR | Errno::XDEV | Errno::MLINK => Failed::Refused,
        other => Failed::Other(other),
    }
}

/// Opens `name` in the folder `dir` without following a link.
fn open_component(dir: &File, name: &str, want_dir: bool, volume: u64) -> Result<File, Failed> {
    let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    if want_dir {
        flags |= OFlags::DIRECTORY;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if !NO_OPENAT2.load(Ordering::Relaxed) {
        use rustix::fs::{ResolveFlags, openat2};
        let resolve = ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV;
        match openat2(dir.as_fd(), name, flags, Mode::empty(), resolve) {
            Ok(fd) => return Ok(File::from(fd)),
            Err(Errno::NOSYS) => NO_OPENAT2.store(true, Ordering::Relaxed),
            Err(other) => return Err(classify(other)),
        }
    }
    let file = File::from(openat(dir.as_fd(), name, flags, Mode::empty()).map_err(classify)?);
    let meta = file.metadata().map_err(|e| Failed::Other(errno_of(&e)))?;
    if volume_of(&meta) != volume {
        return Err(Failed::Refused);
    }
    Ok(file)
}

fn errno_of(error: &std::io::Error) -> Errno {
    Errno::from_raw_os_error(error.raw_os_error().unwrap_or(Errno::IO.raw_os_error()))
}

fn other(errno: Errno) -> ApiError {
    ApiError::Refused(format!(
        "cannot open the path: {}",
        std::io::Error::from(errno)
    ))
}

fn refused() -> ApiError {
    ApiError::Refused(NOT_PLAIN.to_string())
}

/// Opens the component, making the folder first when the use allows it and it is missing.
fn open_or_make(
    dir: &File,
    name: &str,
    want_dir: bool,
    usage: Use,
    volume: u64,
) -> Result<File, ApiError> {
    match open_component(dir, name, want_dir, volume) {
        Ok(file) => Ok(file),
        Err(Failed::Refused) => Err(refused()),
        Err(Failed::Other(errno)) => Err(other(errno)),
        Err(Failed::Missing) if usage == Use::Read => Err(ApiError::Invalid(
            "a path to send does not exist inside its root".to_string(),
        )),
        Err(Failed::Missing) => {
            // Made with no replacing. A folder that appears in between is opened and checked like
            // any other.
            match mkdirat(dir.as_fd(), name, Mode::RWXU | Mode::RWXG | Mode::RWXO) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(errno) => return Err(other(errno)),
            }
            open_component(dir, name, true, volume).map_err(|failed| match failed {
                Failed::Other(errno) => other(errno),
                Failed::Missing | Failed::Refused => refused(),
            })
        }
    }
}

/// Opens every component of `names` below the root, one from the handle of the one before.
pub(in crate::roots) fn walk(
    start: &Start<'_>,
    names: &[&str],
    usage: Use,
) -> Result<Opened, ApiError> {
    let mut path: PathBuf = start.path.to_path_buf();
    let mut held: Vec<File> = Vec::new();
    let mut current = start.held.try_clone().map_err(|e| other(errno_of(&e)))?;
    for (index, name) in names.iter().enumerate() {
        let last = index + 1 == names.len();
        path.push(name);
        if last && usage == Use::FileName {
            return name_only(current, path, name, held);
        }
        let want_dir = !(last && usage == Use::Read);
        let next = open_or_make(&current, name, want_dir, usage, start.volume)?;
        held.push(std::mem::replace(&mut current, next));
    }
    let meta = current.metadata().map_err(|e| other(errno_of(&e)))?;
    if !(meta.is_dir() || meta.is_file()) {
        return Err(refused());
    }
    let found = facts(&current).map_err(|e| other(errno_of(&e)))?;
    // A file is sent from the folder it is in, and a folder from itself.
    let handle = match held.last().filter(|_| meta.is_file()) {
        Some(parent) => parent.try_clone(),
        None => current.try_clone(),
    }
    .map_err(|e| other(errno_of(&e)))?;
    held.push(current);
    Ok(Opened {
        path,
        pin: found.identity,
        links: found.links,
        is_file: meta.is_file(),
        handle,
        name: names.last().map(|n| (*n).to_string()).unwrap_or_default(),
        _held: held,
    })
}

/// The last name of a path whose file the engine creates: it is not opened, and is refused if it is
/// already there as a link.
fn name_only(
    parent: File,
    path: PathBuf,
    name: &str,
    mut held: Vec<File>,
) -> Result<Opened, ApiError> {
    match statat(parent.as_fd(), name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(found) if FileType::from_raw_mode(found.st_mode) == FileType::Symlink => {
            return Err(refused());
        }
        Ok(_) | Err(Errno::NOENT) => {}
        Err(errno) => return Err(other(errno)),
    }
    let found = facts(&parent).map_err(|e| other(errno_of(&e)))?;
    let handle = parent.try_clone().map_err(|e| other(errno_of(&e)))?;
    held.push(parent);
    Ok(Opened {
        path,
        pin: found.identity,
        links: 1,
        is_file: false,
        handle,
        name: name.to_string(),
        _held: held,
    })
}
