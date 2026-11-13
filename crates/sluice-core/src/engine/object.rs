//! Which folder or file a handle names, read from the handle and not from a path, and the opens
//! that do not follow a link. A caller that opened an object by a route it trusts hands the engine
//! its [`ObjectId`], and the engine checks that the object it opens by path is that one.

use std::io;
use std::path::Path;

use crate::Error;

/// What names one file system object: the volume and the object's number on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectId {
    volume: u64,
    // 128 bits, because ReFS and a Dev Drive number their files with that many.
    object: u128,
}

/// An object's identity and how many names it has.
#[derive(Clone, Copy, Debug)]
pub struct Facts {
    /// Which object it is.
    pub identity: ObjectId,
    /// How many names it has.
    pub links: u64,
}

impl ObjectId {
    /// The volume the object is on.
    pub fn volume(&self) -> u64 {
        self.volume
    }

    /// The object a path names, read from a handle opened without following a link in the last
    /// component. A folder and a file both work.
    pub fn of_path(path: &Path) -> io::Result<Self> {
        let handle = match open_folder(path) {
            Ok(handle) => handle,
            // A file is not a folder, which a folder-only open says as an error.
            Err(_) => open_file(path)?,
        };
        Ok(facts(&handle)?.identity)
    }
}

/// The objects a caller opened before it handed the engine their paths. A receive checks them where
/// it first opens the same paths, so a folder swapped for another in between is refused and never
/// written into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pins {
    /// The output folder.
    pub out: Option<ObjectId>,
    /// The folder the receipt file is written in.
    pub receipt_parent: Option<ObjectId>,
    /// Whether the receive's endpoint listens on `127.0.0.1` only.
    pub loopback: bool,
}

/// Refuses a path that no longer names the object that was pinned.
pub(crate) fn check_pinned(path: &Path, pinned: Option<ObjectId>) -> Result<(), Error> {
    let Some(pinned) = pinned else {
        return Ok(());
    };
    match ObjectId::of_path(path) {
        Ok(now) if now == pinned => Ok(()),
        _ => Err(Error::Policy(
            "a folder was replaced after it was checked, so nothing is written there".to_string(),
        )),
    }
}

/// Whether a reparse tag is one of a name surrogate: a symbolic link, a junction or a mount point,
/// which stand for another path. A cloud-file placeholder, a deduplicated file and the other tags
/// stand for the file itself, so they are read as files. This is `IsReparseTagNameSurrogate`: bit
/// 29 of the tag.
pub fn is_name_surrogate_tag(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

#[cfg(unix)]
pub use unix::{facts, is_link, open_file, open_folder, volume_of};

#[cfg(unix)]
mod unix {
    use super::{Facts, ObjectId};
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    /// The object's identity and link count, from the handle.
    pub fn facts(file: &File) -> io::Result<Facts> {
        let meta = file.metadata()?;
        Ok(Facts {
            identity: ObjectId {
                volume: meta.dev(),
                object: u128::from(meta.ino()),
            },
            links: meta.nlink(),
        })
    }

    /// Whether the open object stands for another path. An open without `O_NOFOLLOW`'s refusal
    /// cannot return a link on Unix, so it is never one.
    pub fn is_link(_file: &File) -> io::Result<bool> {
        Ok(false)
    }

    /// The volume a path's metadata says the object is on, for the check that a component stays on
    /// its root's volume. A mount point is a folder on another volume.
    pub fn volume_of(meta: &std::fs::Metadata) -> u64 {
        meta.dev()
    }

    /// A folder opened without following a link in its last component.
    pub fn open_folder(path: &Path) -> io::Result<File> {
        let flags = rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags.bits() as i32)
            .open(path)
    }

    /// A file opened for its identity alone, without following a link in its last component.
    pub fn open_file(path: &Path) -> io::Result<File> {
        // `O_NONBLOCK` so that opening a named pipe returns instead of waiting for a writer.
        let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags.bits() as i32)
            .open(path)
    }
}

#[cfg(windows)]
pub use windows::{facts, is_link, open_file, open_folder, reparse_tag, volume_of};

#[cfg(windows)]
mod windows {
    use super::{Facts, ObjectId};
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FileAttributeTagInfo, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    };

    /// The object's volume serial number, its 128-bit file id (the 64-bit index where the file system
    /// has no other) and its link count, from the handle.
    #[allow(unsafe_code)]
    pub fn facts(file: &File) -> io::Result<Facts> {
        let handle = file.as_raw_handle() as HANDLE;
        // SAFETY: the structure is plain data that the call fills in, and the handle is open for
        // as long as `file` is borrowed.
        let info = unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            if GetFileInformationByHandle(handle, &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
            info
        };
        let index = (u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow);
        let mut wide: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer is the structure the class names, and its size is given.
        let got = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&raw mut wide).cast::<c_void>(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        // A file system with no 128-bit id says so, and its 64-bit index is the id.
        let (volume, object) = if got == 0 {
            (u64::from(info.dwVolumeSerialNumber), index)
        } else {
            (
                wide.VolumeSerialNumber,
                u128::from_le_bytes(wide.FileId.Identifier),
            )
        };
        Ok(Facts {
            identity: ObjectId { volume, object },
            links: u64::from(info.nNumberOfLinks),
        })
    }

    /// Whether the open object is a reparse point of a name surrogate, read from its tag.
    pub fn is_link(file: &File) -> io::Result<bool> {
        Ok(super::is_name_surrogate_tag(reparse_tag(file)?))
    }

    /// The reparse tag of the open object, or zero when it is not a reparse point.
    #[allow(unsafe_code)]
    pub fn reparse_tag(file: &File) -> io::Result<u32> {
        // SAFETY: the buffer is the structure the class names, and its size is given.
        let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let got = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle() as HANDLE,
                FileAttributeTagInfo,
                (&raw mut info).cast::<c_void>(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if got == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(if info.FileAttributes & 0x400 != 0 {
            info.ReparseTag
        } else {
            0
        })
    }

    /// A reparse point is opened as itself and not followed, and a folder needs backup semantics.
    pub fn open_folder(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    /// A file opened as itself, without following a reparse point.
    pub fn open_file(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    /// Windows has no volume number in a path's metadata, so a component is judged a mount point by
    /// being a reparse point, which the caller refuses first. Zero never differs from itself.
    pub fn volume_of(_meta: &std::fs::Metadata) -> u64 {
        0
    }
}

#[cfg(test)]
mod tests;
