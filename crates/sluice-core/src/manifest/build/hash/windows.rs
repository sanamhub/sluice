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

/// Whether a file found by handle is read through its filter: it is a reparse point, and its tag is
/// not that of a name surrogate. A cloud-file placeholder, a deduplicated file and the other tags
/// stand for the file itself, so reading the handle that was opened as the point would give a stub;
/// a link, a junction or a mount point is never read, and a plain file or folder is read as it is.
pub(super) fn needs_filter(attributes: u32, tag: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        && attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        && !crate::engine::is_name_surrogate_tag(tag)
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

/// Opens the same object again, from the handle and without the flag, so the filter gives the
/// content: the file of a confined walk is found by handle and has no path to open a second time.
/// The first handle is held, so the file cannot be renamed or deleted in between.
#[allow(unsafe_code)]
pub(super) fn through_filter_handle(held: File) -> Result<Opened, HashFault> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::ReOpenFile;
    const GENERIC_READ: u32 = 0x8000_0000;
    // SAFETY: the handle is open for as long as `held` is, and the call returns a new handle that
    // this function owns, or the invalid value.
    let raw = unsafe { ReOpenFile(held.as_raw_handle() as HANDLE, GENERIC_READ, SHARE, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(HashFault::Unreadable(
            std::io::Error::last_os_error().kind(),
        ));
    }
    // SAFETY: `raw` is a valid handle that nothing else owns.
    let file = unsafe { File::from_raw_handle(raw as _) };
    let before = file
        .metadata()
        .map_err(|e| HashFault::Unreadable(e.kind()))?;
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
    fn the_tag_decides_which_reparse_points_are_read_through_their_filter() {
        let point = FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_ARCHIVE;
        // Files that stand for themselves: deduplicated, cloud placeholders, projected, WIM.
        for tag in [
            0x8000_0013_u32, // IO_REPARSE_TAG_DEDUP
            0x9000_001A,     // IO_REPARSE_TAG_CLOUD_9
            0x8000_101A,     // OneDrive
            0x9000_001C,     // IO_REPARSE_TAG_PROJFS
            0x8000_0008,     // IO_REPARSE_TAG_WIM
        ] {
            assert!(
                needs_filter(point | FILE_ATTRIBUTE_SPARSE_FILE, tag),
                "{tag:#x}"
            );
        }
        // Name surrogates are never read.
        for tag in [0xA000_000C_u32, 0xA000_0003] {
            assert!(!needs_filter(point, tag), "{tag:#x}");
        }
        // No reparse point, or a folder, is not read through a filter.
        assert!(!needs_filter(FILE_ATTRIBUTE_ARCHIVE, 0));
        assert!(!needs_filter(FILE_ATTRIBUTE_NORMAL, 0x8000_0013));
        assert!(!needs_filter(
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
            0x8000_0013
        ));
    }

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
