//! Windows reparse points that are not links, such as the files of Data Deduplication.
//!
//! The first open uses `FILE_FLAG_OPEN_REPARSE_POINT`, so that a link is opened as itself and not
//! followed. A reparse point that is not a link is then opened as itself too, and its bytes are
//! not the user's content: the filter behind it, which gives the content, never ran. Reading that
//! handle would send other bytes under the file's name with no error. Such a file is opened a
//! second time without the flag and read through its filter, while the first handle is held.
//!
//! OneDrive placeholders should not reach here. Microsoft's Cloud Files documentation says the
//! platform hides its reparse points from every process but sync engines and those whose image is
//! under `%systemroot%`, so a placeholder looks like a plain file, and a read hydrates it. That
//! was not tried on this project's machines.

use std::fs::{File, Metadata, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use super::{HashFault, Opened};

/// Every open shares reading and writing and not deleting. While a handle is open, nobody can
/// rename or delete the file, so its path keeps naming it.
pub(super) const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;

/// What a handle opened with `FILE_FLAG_OPEN_REPARSE_POINT` is.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Handle {
    /// A file with no reparse point, read as it is.
    File,
    /// A file whose reparse point is not a link. Its content is behind a filter.
    ReparsePoint,
    /// A link, a folder, or anything else that is not a file to read.
    Other,
}

pub(super) fn classify(meta: &Metadata) -> Handle {
    decide(meta.file_attributes(), meta.file_type().is_symlink())
}

/// The decision on the attributes alone, so it is tested on a machine with no volume that has
/// such files. `is_link` is what std says of a name-surrogate reparse tag: a symlink, a junction
/// or a mount point.
pub(super) fn decide(attributes: u32, is_link: bool) -> Handle {
    if is_link || attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Handle::Other;
    }
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Handle::ReparsePoint
    } else {
        Handle::File
    }
}

/// Opens `path` again without the flag, so the filter gives the content, while `held`, the first
/// handle, keeps the path from being renamed or deleted. The second handle has to be a regular
/// file as well.
pub(super) fn through_filter(path: &Path, held: File) -> Result<Opened, HashFault> {
    let unreadable = |e: std::io::Error| HashFault::Unreadable(e.kind());
    let file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE)
        .open(path)
        .map_err(unreadable)?;
    let before = file.metadata().map_err(unreadable)?;
    if !before.is_file() {
        return Err(HashFault::Changed);
    }
    Ok(Opened {
        file,
        before,
        _held: Some(held),
    })
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_SPARSE_FILE,
    };

    use super::*;

    #[test]
    fn a_plain_file_is_read_as_it_is() {
        for attributes in [
            FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_ARCHIVE,
            FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_ARCHIVE,
            FILE_ATTRIBUTE_SPARSE_FILE,
        ] {
            assert_eq!(decide(attributes, false), Handle::File, "{attributes:#x}");
        }
    }

    #[test]
    fn a_reparse_point_that_is_not_a_link_is_read_through_its_filter() {
        // Data Deduplication leaves a file sparse with a reparse point.
        let dedup =
            FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_SPARSE_FILE | FILE_ATTRIBUTE_ARCHIVE;
        assert_eq!(decide(dedup, false), Handle::ReparsePoint);
        assert_eq!(
            decide(FILE_ATTRIBUTE_REPARSE_POINT, false),
            Handle::ReparsePoint
        );
    }

    #[test]
    fn a_link_or_a_folder_is_never_read() {
        assert_eq!(decide(FILE_ATTRIBUTE_REPARSE_POINT, true), Handle::Other);
        assert_eq!(decide(FILE_ATTRIBUTE_ARCHIVE, true), Handle::Other);
        assert_eq!(decide(FILE_ATTRIBUTE_DIRECTORY, false), Handle::Other);
        let junction = FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT;
        assert_eq!(decide(junction, false), Handle::Other);
    }

    #[test]
    fn a_plain_file_on_this_disk_is_classified_as_a_file() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"x").expect("write");
        let opened = super::super::open(&path).expect("a regular file opens");
        assert_eq!(classify(&opened.before), Handle::File);
        assert!(opened._held.is_none());
    }

    #[test]
    fn an_open_file_cannot_be_renamed_or_deleted_until_it_is_closed() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"x").expect("write");
        let opened = super::super::open(&path).expect("a regular file opens");
        assert!(std::fs::rename(&path, dir.path().join("b.txt")).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(opened);
        std::fs::rename(&path, dir.path().join("b.txt")).expect("renamed once closed");
    }
}
