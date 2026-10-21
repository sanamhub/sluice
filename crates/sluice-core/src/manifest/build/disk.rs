//! Where each file of a built manifest is on disk.

use std::fmt;
use std::path::{Path, PathBuf};

/// The disk path of each entry of a [`Built`](super::Built) manifest, by index, so a caller reads
/// the files the walk saw. A second walk could find a different folder.
///
/// A disk path is not the entry's `path`: that one is NFC with `/` between components, and the
/// name on disk can be NFD or hold `\`. A disk path holds names, so it is never written to a log
/// or an error, and the `Debug` form gives only the count.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Build, Kind, Manifest};
///
/// let folder = tempfile::tempdir()?;
/// std::fs::create_dir(folder.path().join("docs"))?;
/// std::fs::write(folder.path().join("docs").join("a.txt"), "hello")?;
///
/// let Build::Ready(built) = Manifest::build(folder.path())? else {
///     panic!("nothing is refused");
/// };
/// for (index, entry) in built.manifest.entries().iter().enumerate() {
///     match entry.kind {
///         Kind::File => {
///             let disk = built.disk.file(index).expect("every file has one");
///             assert_eq!(std::fs::read(disk)?, b"hello");
///         }
///         _ => assert!(built.disk.file(index).is_none()),
///     }
/// }
/// assert_eq!(format!("{:?}", built.disk), "DiskPaths { files: 1 }");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct DiskPaths {
    /// One slot per entry, in manifest order. `Some` exactly for a `File` entry.
    paths: Vec<Option<PathBuf>>,
}

impl DiskPaths {
    pub(super) fn new(paths: Vec<Option<PathBuf>>) -> Self {
        Self { paths }
    }

    /// The file on disk for the entry at `index` of `manifest.entries()`. `None` for a folder, a
    /// symlink, or an index past the last entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{Build, Manifest};
    ///
    /// let folder = tempfile::tempdir()?;
    /// std::fs::write(folder.path().join("a.txt"), "hello")?;
    /// let Build::Ready(built) = Manifest::build(folder.path())? else {
    ///     panic!("nothing is refused");
    /// };
    /// assert_eq!(built.disk.file(0), Some(folder.path().join("a.txt").as_path()));
    /// assert_eq!(built.disk.file(1), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn file(&self, index: usize) -> Option<&Path> {
        self.paths.get(index)?.as_deref()
    }
}

impl fmt::Debug for DiskPaths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let files = self.paths.iter().filter(|path| path.is_some()).count();
        f.debug_struct("DiskPaths").field("files", &files).finish()
    }
}
