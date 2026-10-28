//! The disk side of one receive: the space it needs, a private temporary folder, and the errors it
//! reports. The partial store itself is in [`store`], and installing a finished file in [`install`].

mod install;
mod meta;
mod registry;
mod store;

pub use install::install;
pub use meta::MetaStore;
#[cfg(test)]
pub(crate) use registry::remember_as;
pub use registry::{
    clash as registry_clash, discard, find as find_record, forget as forget_record,
    remember as remember_record,
};
#[cfg(test)]
pub use store::MARKER;
pub use store::{claim, identify, partial_dir, recorded, remember};

use std::{
    fs, io,
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
