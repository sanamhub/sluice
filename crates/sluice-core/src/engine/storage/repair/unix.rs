//! The store folder opened once, so the listing, the copy and the rename name the folder that
//! passed the identity check and nothing under a link that was put there later.

use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use iroh_blobs::Hash;
use rustix::{
    fd::OwnedFd,
    fs::{AtFlags, FileType, Mode, OFlags},
    io::Errno,
};

use super::{COPY_SLACK, alien, changed, copy_and_hash};
use crate::{
    Error,
    engine::storage::{FolderId, available_bytes, storage},
};

pub(super) struct Dir {
    fd: OwnedFd,
    path: PathBuf,
}

impl Dir {
    /// Opens `partial` as a folder, without following a link at its name.
    pub(super) fn open(partial: &Path) -> Result<Option<Self>, Error> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        match rustix::fs::open(partial, flags, Mode::empty()) {
            Ok(fd) => Ok(Some(Self {
                fd,
                path: partial.to_owned(),
            })),
            Err(Errno::NOENT) => Ok(None),
            Err(Errno::LOOP) | Err(Errno::NOTDIR) => Err(changed(partial)),
            Err(error) => Err(storage("open", partial, error.into())),
        }
    }

    /// The identity of the folder this handle names, and not of the name it was opened through.
    pub(super) fn identity(&self) -> Result<Option<FolderId>, Error> {
        let stat = rustix::fs::fstat(&self.fd)
            .map_err(|error| storage("read", &self.path, error.into()))?;
        let dev = wide(stat.st_dev);
        let ino = wide(stat.st_ino);
        Ok(Some(FolderId::of((dev, ino))))
    }

    /// Lists, copies and renames through the open folder, so a folder swapped in under the name
    /// after the check is not the folder this touches.
    pub(super) fn sweep(self, partial: &Path) -> Result<(), Error> {
        let data = match openat(&self.fd, OsStr::new("data"), OFlags::DIRECTORY) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(()),
            Err(Errno::LOOP) | Err(Errno::NOTDIR) => return Err(alien(partial)),
            Err(error) => return Err(storage("open", &partial.join("data"), error.into())),
        };
        let at = partial.join("data");
        let mut entries = rustix::fs::Dir::read_from(&data)
            .map_err(|error| storage("read", &at, error.into()))?;
        let mut shared = Vec::new();
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|error| storage("read", &at, error.into()))?;
            let Ok(text) = std::str::from_utf8(entry.file_name().to_bytes()) else {
                continue;
            };
            if text == "." || text == ".." {
                continue;
            }
            let name = OsString::from(text);
            // A copy that a killed command left halfway. The name is derived from the data file,
            // so it is never a blob of its own. Unlinking removes the entry and never what a link
            // leads to, so a link planted under this name is taken away rather than followed.
            if text.ends_with(".store") {
                let _ = rustix::fs::unlinkat(&data, &name, AtFlags::empty());
                continue;
            }
            let Some(hex) = text.strip_suffix(".data") else {
                continue;
            };
            let Ok(hash) = hex.parse::<Hash>() else {
                continue;
            };
            if let Some(size) = shared_size(&data, &name) {
                shared.push((name, hash, size));
            }
        }
        for (name, hash, size) in shared {
            give_back(&data, partial, &name, hash, size);
        }
        Ok(())
    }
}

/// The size of `name` when it is a regular file with more than one name, and `None` otherwise.
fn shared_size(data: &OwnedFd, name: &OsStr) -> Option<u64> {
    let file = openat(data, name, OFlags::empty()).ok()?;
    let stat = rustix::fs::fstat(&file).ok()?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return None;
    }
    if wide(stat.st_nlink) <= 1 {
        return None;
    }
    Some(wide(stat.st_size))
}

/// Copies the store's bytes to a name of their own and puts that name in place of the shared one.
/// Nothing is installed when the bytes do not hash to the root they are filed under: those are the
/// user's bytes now, and the store's name for them cannot be trusted.
fn give_back(data: &OwnedFd, partial: &Path, name: &OsStr, hash: Hash, size: u64) {
    if available_bytes(partial).is_ok_and(|free| free < size.saturating_add(COPY_SLACK)) {
        return;
    }
    let Some(stem) = name
        .to_str()
        .map(|text| &text[..text.len() - ".data".len()])
    else {
        return;
    };
    let copy = OsString::from(format!("{stem}.store"));
    let Ok(source) = openat(data, name, OFlags::empty()) else {
        return;
    };
    let Ok(target) = create_copy(data, &copy) else {
        return;
    };
    let mut source = fs::File::from(source);
    let mut target = fs::File::from(target);
    let copied = copy_and_hash(&mut source, &mut target, hash).unwrap_or(false);
    let synced = target.sync_all().is_ok();
    drop(target);
    drop(source);
    if copied && synced && rustix::fs::renameat(data, &copy, data, name).is_ok() {
        return;
    }
    let _ = rustix::fs::unlinkat(data, &copy, AtFlags::empty());
}

/// Makes the copy at a name that must not be there yet, without following a link at it. A copy
/// that an earlier run left behind is removed first, so a link at that name is never written
/// through and never blocks the repair for good.
fn create_copy(data: &OwnedFd, copy: &OsStr) -> Result<OwnedFd, Errno> {
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    match rustix::fs::openat(data, copy, flags, Mode::from_raw_mode(0o600)) {
        Ok(fd) => Ok(fd),
        Err(Errno::EXIST) => {
            rustix::fs::unlinkat(data, copy, AtFlags::empty())?;
            rustix::fs::openat(data, copy, flags, Mode::from_raw_mode(0o600))
        }
        Err(error) => Err(error),
    }
}

/// The data file relative to the store folder, or a name created without following a link.
fn openat(data: &OwnedFd, name: &OsStr, extra: OFlags) -> Result<OwnedFd, Errno> {
    let flags = extra | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    rustix::fs::openat(data, name, flags, Mode::empty())
}

/// A field of `Stat` as `u64`, whatever integer width the platform gives it. The widths differ
/// between the Unixes even for one field, so the conversion is written once here.
fn wide<T: TryInto<u64>>(value: T) -> u64 {
    value.try_into().unwrap_or(0)
}
