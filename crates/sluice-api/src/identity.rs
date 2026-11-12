//! Which folder or file a handle names, read from the handle and not from a path, and the open of a
//! root's folder without following a link.

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::path::Path;

/// What names one file system object: the volume and the object's number on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    volume: u64,
    object: u64,
}

/// An object's identity and how many names it has.
pub(crate) struct Facts {
    pub identity: Identity,
    pub links: u64,
}

#[cfg(unix)]
pub(crate) fn facts(file: &File) -> io::Result<Facts> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    Ok(Facts {
        identity: Identity {
            volume: meta.dev(),
            object: meta.ino(),
        },
        links: meta.nlink(),
    })
}

/// The volume of the folder a path names, for the check that a component stays on its root's
/// volume. A mount point is a folder on another volume.
#[cfg(unix)]
pub(crate) fn volume_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

/// A folder opened without following a link in its last component.
#[cfg(unix)]
pub(crate) fn open_folder(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let flags = rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)
}

/// A file opened for its identity alone, without following a link in its last component.
#[cfg(unix)]
pub(crate) fn open_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    // `O_NONBLOCK` so that opening a named pipe returns instead of waiting for a writer.
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)
}

#[cfg(windows)]
pub(crate) use windows::{facts, open_file, open_folder, volume_of};

#[cfg(windows)]
mod windows {
    use super::{Facts, Identity};
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        GetFileInformationByHandle,
    };

    /// The object's volume serial number, its 64-bit index and its link count, from the handle.
    #[allow(unsafe_code)]
    pub(crate) fn facts(file: &File) -> io::Result<Facts> {
        // SAFETY: the structure is plain data that the call fills in, and the handle is open for
        // as long as `file` is borrowed.
        let info = unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            if GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
            info
        };
        Ok(Facts {
            identity: Identity {
                volume: u64::from(info.dwVolumeSerialNumber),
                object: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            },
            links: u64::from(info.nNumberOfLinks),
        })
    }

    /// A reparse point is opened as itself and not followed, and a folder needs backup semantics.
    pub(crate) fn open_folder(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    pub(crate) fn open_file(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    /// Windows has no volume number in a path's metadata, so a component is judged a mount point by
    /// being a reparse point, which the caller refuses first. Zero never differs from itself.
    pub(crate) fn volume_of(_meta: &std::fs::Metadata) -> u64 {
        0
    }
}
