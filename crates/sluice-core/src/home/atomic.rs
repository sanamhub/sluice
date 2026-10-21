//! Writing a small file so that a crash leaves the old content or the new, never half of either.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::storage;
use crate::Error;

/// Writes `bytes` to `dir/name` through a temporary file in the same folder: write, `fsync`,
/// rename, then `fsync` the folder where the platform allows it, so the rename itself survives a
/// power cut. The temporary file is created private from the start, because a secret written
/// with the default mode and then restricted is readable in between.
///
/// An existing `dir/name` is replaced. A caller that must not replace checks first, under the lock.
///
/// The file keeps the access of the folder it is in on Windows. Use [`write_atomic_secret`] for a
/// file that must not.
pub(super) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    write_with(dir, name, bytes, |_| Ok(()))
}

/// Like [`write_atomic`], for a secret. On Windows the file gets an access list of its own before
/// the bytes are written, whatever the folder allows. On Unix the mode 0600 that every file here
/// is created with already does it.
pub(super) fn write_atomic_secret(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    write_with(dir, name, bytes, keep_private)
}

/// `restrict` runs on the new temporary file after it is created and before anything is written.
fn write_with(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    restrict: fn(&File) -> io::Result<()>,
) -> Result<(), Error> {
    let temporary = dir.join(format!("{name}.tmp"));
    // Left by a process that died between the create and the rename. Only the holder of the
    // home's lock writes here, so it is nobody else's.
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(storage("remove the unfinished", &temporary, e)),
    }
    let result = write_and_rename(dir, name, &temporary, bytes, restrict);
    if result.is_err() {
        // Best effort. The next call removes it too.
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_and_rename(
    dir: &Path,
    name: &str,
    temporary: &Path,
    bytes: &[u8],
    restrict: fn(&File) -> io::Result<()>,
) -> Result<(), Error> {
    let mut file = create_private(temporary).map_err(|e| storage("create", temporary, e))?;
    restrict(&file).map_err(|e| storage("restrict access to", temporary, e))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| storage("write", temporary, e))?;
    drop(file);
    let target = dir.join(name);
    fs::rename(temporary, &target).map_err(|e| storage("rename into place", &target, e))?;
    sync_dir(dir).map_err(|e| storage("sync the folder of", &target, e))
}

/// A new file only the owner can read, with the mode set by the `open` call itself. On Windows
/// the open asks for `WRITE_DAC` as well, which is what `keep_private` needs to set the access list.
fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_WRITE, WRITE_DAC};
        options.access_mode(FILE_GENERIC_WRITE | WRITE_DAC);
    }
    options.open(path)
}

#[cfg(windows)]
fn keep_private(file: &File) -> io::Result<()> {
    super::private_windows::restrict_file(file)
}

#[cfg(not(windows))]
fn keep_private(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Windows has no call to flush a folder through a handle that std can open, and NTFS journals the
/// rename.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}
