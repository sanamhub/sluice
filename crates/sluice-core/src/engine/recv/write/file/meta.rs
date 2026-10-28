//! One file.s mode and time, set through the handle the file was opened with.

use std::{
    fs,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

use tokio::sync::mpsc::UnboundedSender;

use super::super::super::{Event, emit, storage};
use crate::Error;

/// Restores one modification time, or says why it was skipped. A time the platform cannot hold is
/// not a reason to fail a transfer whose bytes are already correct.
pub(super) fn set_time(
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
pub(super) fn set_mode(file: &fs::File, path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if mode == 0 {
        return Ok(());
    }
    // Setuid and setgid are never applied, whatever the sender wrote. Owner read and write always
    // are: a file without them cannot be read back for a resumed receive to compare, and a resume
    // that cannot read what it wrote would fail on a file that is already correct.
    let mode = (mode & 0o777) | 0o600;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(windows)]
pub(super) fn set_mode(file: &fs::File, path: &Path, mode: u32) -> Result<(), Error> {
    if mode == 0 {
        return Ok(());
    }
    // A mode with owner write is never made read-only, for the same reason as the Unix clamp: the
    // file has to stay readable and replaceable by the receive that wrote it.
    let mode = mode | 0o600;
    let mut permissions = file
        .metadata()
        .map_err(|e| storage::policy_path("read the mode of", path, e))?
        .permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    file.set_permissions(permissions)
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(not(any(unix, windows)))]
pub(super) fn set_mode(_file: &fs::File, _path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

/// A folder's mode, set after its children exist so their creation cannot change it.
#[cfg(unix)]
pub(crate) fn set_mode_path(path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if mode == 0 {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(windows)]
pub(crate) fn set_mode_path(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn set_mode_path(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

/// A folder's time. Windows does not restore folder times, so this is a no-op there.
#[cfg(unix)]
pub(crate) fn set_time_path(
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
pub(crate) fn set_time_path(
    _path: &Path,
    _mtime_ms: u64,
    _events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    Ok(())
}
