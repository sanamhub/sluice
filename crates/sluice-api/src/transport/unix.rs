//! The Unix socket: a socket file in a folder of mode 0700 inside the home. A peer is this user or
//! it is refused, by the id the kernel reports for the process at the other end.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use super::{Incoming, accepting};
use crate::error::ApiError;

/// The folder of the socket, inside the home.
const FOLDER: &str = "run";

/// The socket's file name.
const SOCKET: &str = "control.sock";

/// The longest socket path every Unix accepts: `sun_path` holds 104 bytes on macOS and 108 on Linux,
/// and a terminating NUL.
const MAX_PATH_BYTES: usize = 100;

/// The stream of an accepted connection.
pub(crate) type Stream = UnixStream;

/// Where the socket of the home at `home` is.
pub(crate) fn target(home: &Path) -> OsString {
    home.join(FOLDER).join(SOCKET).into_os_string()
}

/// What to show a user of where the daemon listens.
pub(crate) fn describe(target: &OsString) -> String {
    Path::new(target).display().to_string()
}

/// Connects to the daemon at `target`.
pub(crate) async fn open(target: &OsString) -> io::Result<UnixStream> {
    UnixStream::connect(Path::new(target)).await
}

/// The socket file of a running daemon. It removes the file when it drops, if the file is still the
/// one this daemon made.
pub(crate) struct Guard {
    path: PathBuf,
    id: (u64, u64),
}

impl Drop for Guard {
    fn drop(&mut self) {
        let same =
            fs::symlink_metadata(&self.path).is_ok_and(|meta| (meta.dev(), meta.ino()) == self.id);
        if same {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The user this process runs as.
fn own_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Whether the peer's user id is this user's. Takes both as values so a test can try another one.
pub(crate) fn check_uid(peer: u32, own: u32) -> Result<(), ApiError> {
    if peer == own {
        Ok(())
    } else {
        Err(ApiError::Refused(format!(
            "a connection from user {peer} was refused: this daemon serves user {own}"
        )))
    }
}

/// Binds the socket and starts accepting. The peer's id comes from the kernel: tokio reads it with
/// `SO_PEERCRED` on Linux and `getpeereid` on macOS.
pub(crate) fn listen(home: &Path) -> Result<(Incoming<UnixStream>, Guard, String), ApiError> {
    let path = PathBuf::from(target(home));
    if path.as_os_str().len() >= MAX_PATH_BYTES {
        return Err(ApiError::Invalid(format!(
            "the home's path is too long for a socket: {} must be under {MAX_PATH_BYTES} bytes",
            path.display()
        )));
    }
    let folder = path.parent().unwrap_or(Path::new("."));
    prepare_folder(folder).map_err(|e| unusable(folder, &e))?;
    take_over(&path)?;
    let listener = UnixListener::bind(&path).map_err(|e| unusable(&path, &e))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|e| unusable(&path, &e))?;
    let meta = fs::symlink_metadata(&path).map_err(|e| unusable(&path, &e))?;
    let guard = Guard {
        path: path.clone(),
        id: (meta.dev(), meta.ino()),
    };
    // Shared with each accept, so the future an accept returns owns what it waits on.
    let listener = std::sync::Arc::new(listener);
    let incoming = accepting(move || {
        let listener = listener.clone();
        async move {
            let (stream, _) = listener.accept().await?;
            let uid = stream.peer_cred()?.uid();
            match check_uid(uid, own_uid()) {
                Ok(()) => Ok(Some(stream)),
                Err(refused) => {
                    tracing::warn!("{refused}");
                    Ok(None)
                }
            }
        }
    });
    Ok((incoming, guard, path.display().to_string()))
}

/// Makes the folder if it is missing, with mode 0700, and refuses one that is a link, that another
/// user owns, or that others can reach.
fn prepare_folder(folder: &Path) -> io::Result<()> {
    match fs::symlink_metadata(folder) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(folder)
        }
        Err(e) => Err(e),
        Ok(meta) => {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(other("it is not a plain folder"));
            }
            if meta.uid() != own_uid() {
                return Err(other("another user owns it"));
            }
            if meta.mode() & 0o077 != 0 {
                return Err(other("others can reach it; its mode must be 0700"));
            }
            Ok(())
        }
    }
}

/// Takes over a socket path that is left from a daemon that is gone, and refuses any other thing
/// that is there: a link, a file that is not a socket, a socket another user made, and a socket a
/// live daemon answers on.
fn take_over(path: &Path) -> Result<(), ApiError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(unusable(path, &e)),
    };
    if meta.file_type().is_symlink() {
        return Err(refused(path, "it is a link, which is never followed"));
    }
    if !meta.file_type().is_socket() {
        return Err(refused(path, "it is there and is not a socket"));
    }
    if meta.uid() != own_uid() {
        return Err(refused(path, "another user made it"));
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => Err(refused(path, "a daemon is already listening on it")),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            fs::remove_file(path).map_err(|e| unusable(path, &e))
        }
        Err(e) => Err(unusable(path, &e)),
    }
}

fn other(why: &str) -> io::Error {
    io::Error::other(why.to_string())
}

fn refused(path: &Path, why: &str) -> ApiError {
    ApiError::Refused(format!(
        "the socket path {} is not used: {why}",
        path.display()
    ))
}

fn unusable(path: &Path, error: &io::Error) -> ApiError {
    ApiError::Failed(format!(
        "cannot use {} for the socket: {error}",
        path.display()
    ))
}

#[cfg(test)]
mod tests;
