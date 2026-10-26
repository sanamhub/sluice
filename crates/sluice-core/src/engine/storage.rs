//! The disk side of one receive: the space it needs, where its partial store lives, and installing
//! the finished file without a second copy.

use std::{
    fs::File,
    io,
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

/// The hidden store beside the destination. A per-transfer store keeps partial data on the same
/// filesystem as the file it will become, so a finished blob can be moved into place.
pub fn partial_dir(out: &Path, root: &str) -> PathBuf {
    let short: String = root.chars().take(16).collect();
    out.join(format!(".sluice-partial-{short}"))
}

/// The temporary name the finished file gets before it replaces the destination. It is in the
/// same folder, so the last step is a rename.
pub fn incoming_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    target.with_file_name(format!(".{name}.sluice-new"))
}

/// Moves `from` to `target`, replacing it when `force` is set. The caller checks the clash before
/// the transfer starts; this is the last step, after the bytes are verified.
pub fn install(from: &Path, target: &Path, force: bool) -> Result<(), Error> {
    if target.exists() && !force {
        return Err(clash(target));
    }
    // Windows replaces an existing file in `rename`. A read-only destination is the one case it
    // cannot, and then only the read-only bit is cleared before the same rename is tried again.
    #[cfg(windows)]
    {
        match std::fs::rename(from, target) {
            Ok(()) => Ok(()),
            Err(e) if target.exists() => {
                let meta =
                    std::fs::symlink_metadata(target).map_err(|e| storage("read", target, e))?;
                if meta.permissions().readonly() {
                    let mut permissions = meta.permissions();
                    // This is Windows, where readonly is the only permission bit `set_permissions`
                    // holds; the Unix warning the lint names does not apply.
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    std::fs::set_permissions(target, permissions)
                        .map_err(|e| storage("replace", target, e))?;
                    std::fs::rename(from, target).map_err(|e| storage("install", target, e))
                } else {
                    Err(storage("install", target, e))
                }
            }
            Err(e) => Err(storage("install", target, e)),
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, target).map_err(|e| storage("install", target, e))
    }
}

/// The one clash message, naming the first path and both ways out.
pub fn clash(target: &Path) -> Error {
    Error::Policy(format!(
        "{} already exists; pass --force to replace it, or choose another --out",
        target.display()
    ))
}

/// `fsync` the installed file, so its bytes reach the disk before the command says done.
pub fn sync_file(path: &Path) -> Result<(), Error> {
    let file = File::options()
        .write(true)
        .open(path)
        .map_err(|e| storage("open", path, e))?;
    file.sync_all().map_err(|e| storage("sync", path, e))
}

pub fn storage(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Storage(format!("cannot {doing} {}: {source}", path.display()))
}

pub fn policy_path(doing: &str, path: &Path, source: io::Error) -> Error {
    Error::Policy(format!("cannot {doing} {}: {source}", path.display()))
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
mod tests {
    use super::{incoming_path, needed_bytes, partial_dir};
    use std::path::Path;

    #[test]
    fn a_hundred_gigabytes_needs_about_a_hundred_and_not_two_hundred() {
        let need = needed_bytes(100 << 30);
        assert!(need > 100 << 30);
        assert!(need < 101 << 30, "{need}");
    }

    #[test]
    fn the_partial_store_is_hidden_beside_the_output() {
        let path = partial_dir(Path::new("/out"), "0123456789abcdef0123");
        assert_eq!(path, Path::new("/out/.sluice-partial-0123456789abcdef"));
    }

    #[test]
    fn the_temporary_name_keeps_the_file_name() {
        assert_eq!(
            incoming_path(Path::new("/out/a.bin")),
            Path::new("/out/.a.bin.sluice-new")
        );
    }

    /// The install step is one rename because Windows already replaces an existing target. This
    /// keeps the test next to the claim, so a platform that changes its mind fails here.
    #[cfg(windows)]
    #[test]
    fn a_rename_replaces_a_file_that_is_already_there() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let incoming = dir.path().join("incoming");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&incoming, b"new").unwrap();
        super::install(&incoming, &target, true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!incoming.exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_read_only_target_is_replaced_by_clearing_the_bit() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let incoming = dir.path().join("incoming");
        std::fs::write(&target, b"old").unwrap();
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&target, permissions).unwrap();
        std::fs::write(&incoming, b"new").unwrap();
        super::install(&incoming, &target, true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }
}
