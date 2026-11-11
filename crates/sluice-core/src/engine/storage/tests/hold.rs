//! A folder a test keeps a removal out of, so the code that runs when a removal gives up is
//! exercised without waiting on another process.

use std::path::Path;

/// A folder a removal cannot take, released when it is dropped: no write permission on the folder
/// itself on Unix, and a file in it that another handle does not share on Windows.
///
/// The superuser is stopped by no mode on Unix, so `in_folder` answers `None` there: a test that
/// needs a folder held says it was skipped, and does not pass as if the removal had given up.
pub(crate) struct Held {
    #[cfg(unix)]
    folder: std::path::PathBuf,
    #[cfg(windows)]
    _file: std::fs::File,
}

impl Held {
    #[cfg(unix)]
    pub(crate) fn in_folder(folder: &Path) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;

        if rustix::process::geteuid().is_root() {
            return None;
        }
        let mut mode = std::fs::metadata(folder).unwrap().permissions();
        mode.set_mode(0o500);
        std::fs::set_permissions(folder, mode).unwrap();
        Some(Self {
            folder: folder.to_path_buf(),
        })
    }

    #[cfg(windows)]
    pub(crate) fn in_folder(folder: &Path) -> Option<Self> {
        use std::os::windows::fs::OpenOptionsExt;

        let path = folder.join("held");
        std::fs::write(&path, b"held").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        Some(Self { _file: file })
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn in_folder(_folder: &Path) -> Option<Self> {
        None
    }
}

impl Drop for Held {
    #[cfg(unix)]
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        let mut mode = std::fs::metadata(&self.folder).unwrap().permissions();
        mode.set_mode(0o700);
        let _ = std::fs::set_permissions(&self.folder, mode);
    }

    #[cfg(windows)]
    fn drop(&mut self) {}
}
