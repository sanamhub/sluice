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
pub(super) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    let temporary = dir.join(format!("{name}.tmp"));
    // Left by a process that died between the create and the rename. Only the holder of the
    // home's lock writes here, so it is nobody else's.
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(storage("remove the unfinished", &temporary, e)),
    }
    let result = write_and_rename(dir, name, &temporary, bytes);
    if result.is_err() {
        // Best effort. The next call removes it too.
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_and_rename(dir: &Path, name: &str, temporary: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = create_private(temporary).map_err(|e| storage("create", temporary, e))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| storage("write", temporary, e))?;
    drop(file);
    let target = dir.join(name);
    fs::rename(temporary, &target).map_err(|e| storage("rename into place", &target, e))?;
    sync_dir(dir).map_err(|e| storage("sync the folder of", &target, e))
}

/// A new file only the owner can read, with the mode set by the `open` call itself.
fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
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
