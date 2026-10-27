//! One file's install: a private temporary name, its metadata set through the open handle, one
//! rename into place, and the content comparison that lets a resumed receive skip a file it wrote.

use std::{
    fs,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

use iroh_blobs::{Hash, api::blobs::BlobReader};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::mpsc::UnboundedSender,
};

use super::super::{Event, emit, storage};
use crate::{
    Error,
    manifest::{Entry, Loc},
};

/// The metadata of one entry, kept together so the write helpers have fewer arguments.
#[derive(Clone, Copy)]
pub(super) struct Meta {
    pub mtime_ms: u64,
    pub mode: u32,
}

/// An empty file, made with `create_new` so a name that appeared under the temporary name is an
/// error and not something this write opens.
pub(super) fn create_empty(path: &Path) -> Result<(), Error> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| storage::storage("create", path, e))?;
    Ok(())
}

/// Applies the metadata of a file this write just made and syncs it before the rename.
pub(super) fn finish(
    path: &Path,
    meta: Meta,
    no_times: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| storage::storage("open", path, e))?;
    apply(&file, path, meta, no_times, events)?;
    file.sync_all()
        .map_err(|e| storage::storage("sync", path, e))
}

/// Sets the time and mode on a file that is already in place, which is what a resumed receive does
/// for a file whose content it recognized.
pub(super) fn touch(
    path: &Path,
    meta: Meta,
    no_times: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let file = fs::File::open(path).map_err(|e| storage::storage("open", path, e))?;
    apply(&file, path, meta, no_times, events)
}

/// Mode and time on the open file, before it has its final name.
pub(super) fn apply(
    file: &fs::File,
    path: &Path,
    meta: Meta,
    no_times: bool,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    if !no_times {
        set_time(file, path, meta.mtime_ms, events)?;
    }
    set_mode(file, path, meta.mode)
}

/// Copies one `Packed` entry out of its pack into `incoming`, and returns the open file so its
/// metadata can be set before the rename.
pub(super) async fn copy_from_pack(
    reader: &mut BlobReader,
    entry: &Entry,
    incoming: &Path,
) -> Result<fs::File, Error> {
    let Loc::Packed { offset, len, .. } = entry.loc else {
        return Err(Error::Bug("a packed entry has no pack range".to_string()));
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(incoming)
        .await
        .map_err(|e| storage::storage("create", incoming, e))?;
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read a pack", incoming, e))?;
    let copied = tokio::io::copy(&mut reader.take(len), &mut file)
        .await
        .map_err(|e| storage::storage("write", incoming, e))?;
    file.flush()
        .await
        .map_err(|e| storage::storage("write", incoming, e))?;
    let file = file.into_std().await;
    if copied != len {
        return Err(Error::Verify(format!(
            "a packed file has {copied} bytes and the manifest says {len}"
        )));
    }
    Ok(file)
}

/// Whether `target` already holds the bytes of this pack range, which is how a receive that was
/// killed after writing a file recognises it instead of refusing it as a clash.
pub(super) async fn same_packed(
    reader: &mut BlobReader,
    entry: &Entry,
    target: &Path,
) -> Result<bool, Error> {
    let Loc::Packed { offset, len, .. } = entry.loc else {
        return Ok(false);
    };
    let existing = fs::read(target).map_err(|e| storage::storage("read", target, e))?;
    if existing.len() as u64 != len {
        return Ok(false);
    }
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read a pack", target, e))?;
    let mut expected = vec![0u8; existing.len()];
    reader
        .read_exact(&mut expected)
        .await
        .map_err(|e| storage::storage("read a pack", target, e))?;
    Ok(existing == expected)
}

/// Whether a target already holds exactly this blob's content.
pub(super) fn same_blob(target: &Path, hash: Hash, size: u64) -> Result<bool, Error> {
    let meta = fs::symlink_metadata(target).map_err(|e| storage::storage("read", target, e))?;
    if !meta.is_file() || meta.len() != size {
        return Ok(false);
    }
    if size == 0 {
        return Ok(true);
    }
    let mut file = fs::File::open(target).map_err(|e| storage::storage("read", target, e))?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| storage::storage("read", target, e))?;
    Ok(*hasher.finalize().as_bytes() == *hash.as_bytes())
}

/// Restores one modification time, or says why it was skipped. A time the platform cannot hold is
/// not a reason to fail a transfer whose bytes are already correct.
fn set_time(
    file: &fs::File,
    path: &Path,
    mtime_ms: u64,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    if mtime_ms == 0 {
        return Ok(());
    }
    let Some(time) = UNIX_EPOCH.checked_add(Duration::from_millis(mtime_ms)) else {
        emit(
            events,
            Event::Warning {
                message: "a modification time is too far in the future for this platform, so it \
                          was not restored"
                    .to_string(),
            },
        );
        return Ok(());
    };
    file.set_times(fs::FileTimes::new().set_modified(time))
        .map_err(|e| storage::storage("set the modification time of", path, e))
}

#[cfg(unix)]
fn set_mode(file: &fs::File, path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if mode == 0 {
        return Ok(());
    }
    // Setuid and setgid are never applied, whatever the sender wrote.
    let mode = mode & 0o777;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(windows)]
fn set_mode(file: &fs::File, path: &Path, mode: u32) -> Result<(), Error> {
    if mode == 0 {
        return Ok(());
    }
    let mut permissions = file
        .metadata()
        .map_err(|e| storage::policy_path("read the mode of", path, e))?
        .permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    file.set_permissions(permissions)
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(not(any(unix, windows)))]
fn set_mode(_file: &fs::File, _path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

/// A folder's mode, set after its children exist so their creation cannot change it.
#[cfg(unix)]
pub(super) fn set_mode_path(path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if mode == 0 {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(windows)]
pub(super) fn set_mode_path(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn set_mode_path(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

/// A folder's time. Windows does not restore folder times, so this is a no-op there.
#[cfg(unix)]
pub(super) fn set_time_path(
    path: &Path,
    mtime_ms: u64,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    if mtime_ms == 0 {
        return Ok(());
    }
    let file = fs::File::open(path).map_err(|e| storage::storage("open", path, e))?;
    set_time(&file, path, mtime_ms, events)
}

#[cfg(not(unix))]
pub(super) fn set_time_path(
    _path: &Path,
    _mtime_ms: u64,
    _events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    Ok(())
}
