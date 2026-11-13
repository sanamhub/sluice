//! Folder handles whose every open is relative to the handle and never follows a link. A name is
//! opened from the folder it is in, one component at a time, so a folder or an ancestor that is
//! swapped for a link while a walk runs cannot lead the walk anywhere but into the object it holds.
//!
//! On Unix an open is `openat` with `O_NOFOLLOW` from the folder's descriptor. On Windows it is
//! `NtCreateFile` with the folder as its `RootDirectory` and `FILE_OPEN_REPARSE_POINT`, so a link
//! is opened as itself and not followed.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;

/// What a name in a folder is, as the folder itself says, without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Dir,
    File,
    /// A symbolic link, or on Windows a junction or any other name surrogate.
    Link,
    Other,
}

/// Whether `name` is one component: not empty, not `.` or `..`, and holding no separator. Every name
/// opened from a folder handle is one, and the walk asks the folder for it by a name it listed.
fn is_one_component(name: &OsStr) -> bool {
    let text = name.to_string_lossy();
    !text.is_empty() && text != "." && text != ".." && !text.contains(['/', '\\'])
}

/// An open folder.
#[derive(Debug)]
pub(super) struct Dir(File);

impl Dir {
    pub(super) fn from_file(folder: File) -> Self {
        Self(folder)
    }

    pub(super) fn into_file(self) -> File {
        self.0
    }

    pub(super) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self(self.0.try_clone()?))
    }
}

#[cfg(unix)]
mod imp {
    use std::os::unix::ffi::OsStringExt;

    use rustix::fs::{AtFlags, FileType, Mode, OFlags, statat};

    use super::{Dir, File, Kind, OsStr, OsString, io};

    impl Dir {
        fn open_at(&self, name: &OsStr, extra: OFlags) -> io::Result<File> {
            debug_assert!(
                super::is_one_component(name),
                "{name:?} is not one component"
            );
            let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | extra;
            Ok(File::from(rustix::fs::openat(
                &self.0,
                name,
                flags,
                Mode::empty(),
            )?))
        }

        /// The folder `name`, which fails when it is a link or not a folder.
        pub(in crate::manifest::build) fn open_dir(&self, name: &OsStr) -> io::Result<Dir> {
            Ok(Dir(self.open_at(name, OFlags::DIRECTORY)?))
        }

        /// The file `name`, which fails when it is a link. `O_NONBLOCK` so that a FIFO opens at once.
        pub(in crate::manifest::build) fn open_file(&self, name: &OsStr) -> io::Result<File> {
            self.open_at(name, OFlags::NONBLOCK)
        }

        pub(in crate::manifest::build) fn kind(&self, name: &OsStr) -> io::Result<Kind> {
            let found = statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW)?;
            Ok(match FileType::from_raw_mode(found.st_mode) {
                FileType::Directory => Kind::Dir,
                FileType::RegularFile => Kind::File,
                FileType::Symlink => Kind::Link,
                _ => Kind::Other,
            })
        }

        /// The names in the folder, which the caller opens one by one from the folder.
        pub(in crate::manifest::build) fn names(&self) -> io::Result<Vec<OsString>> {
            let mut names = Vec::new();
            for entry in rustix::fs::Dir::read_from(&self.0)? {
                let bytes = entry?.file_name().to_bytes().to_vec();
                if bytes != b"." && bytes != b".." {
                    names.push(OsString::from_vec(bytes));
                }
            }
            Ok(names)
        }
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code)]

    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::ptr::{null, null_mut};

    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_BOTH_DIR_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdBothDirectoryInfo,
        FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    use super::{Dir, File, Kind, OsStr, OsString, io};
    use crate::engine::is_link;

    /// `FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE`: what reading a file or listing a folder
    /// needs.
    const ACCESS: u32 = 0x1 | 0x80 | 0x10_0000;
    /// `FILE_READ_ATTRIBUTES | SYNCHRONIZE`: all that asking what a name is needs, so a file that
    /// cannot be read can still be told apart from a link.
    const LOOK: u32 = 0x80 | 0x10_0000;
    const NOT_FOUND: [i32; 2] = [0xC000_0034_u32 as i32, 0xC000_003A_u32 as i32];

    impl Dir {
        fn open_rel(&self, name: &OsStr, options: u32, access: u32) -> io::Result<File> {
            debug_assert!(
                super::is_one_component(name),
                "{name:?} is not one component"
            );
            let wide: Vec<u16> = name.encode_wide().collect();
            let bytes = u16::try_from(wide.len() * 2)
                .map_err(|_| io::Error::other("a name is too long"))?;
            let text = UNICODE_STRING {
                Length: bytes,
                MaximumLength: bytes,
                Buffer: wide.as_ptr().cast_mut(),
            };
            let attributes = OBJECT_ATTRIBUTES {
                Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
                RootDirectory: self.0.as_raw_handle() as HANDLE,
                ObjectName: &text,
                Attributes: OBJ_CASE_INSENSITIVE,
                SecurityDescriptor: null(),
                SecurityQualityOfService: null(),
            };
            let mut handle: HANDLE = null_mut();
            let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
            // SAFETY: every pointer is to a value that lives for the call, and the root directory
            // is open for as long as `self` is borrowed. A returned handle is owned below.
            let result = unsafe {
                NtCreateFile(
                    &mut handle,
                    access,
                    &attributes,
                    &mut status,
                    null(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    FILE_OPEN,
                    options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
                    null::<c_void>(),
                    0,
                )
            };
            if result < 0 {
                return Err(if NOT_FOUND.contains(&result) {
                    io::Error::from(io::ErrorKind::NotFound)
                } else {
                    io::Error::other(format!("the open was refused ({result:#x})"))
                });
            }
            // SAFETY: the call made this handle and nothing else owns it.
            Ok(unsafe { File::from_raw_handle(handle as _) })
        }

        /// The folder `name`, which fails when it is a link or not a folder.
        pub(in crate::manifest::build) fn open_dir(&self, name: &OsStr) -> io::Result<Dir> {
            let folder = self.open_rel(name, FILE_DIRECTORY_FILE, ACCESS)?;
            if is_link(&folder)? {
                return Err(io::Error::other("it is a link"));
            }
            Ok(Dir(folder))
        }

        /// The file `name`, which fails when it is not a file or is a link.
        pub(in crate::manifest::build) fn open_file(&self, name: &OsStr) -> io::Result<File> {
            let file = self.open_rel(name, FILE_NON_DIRECTORY_FILE, ACCESS)?;
            if is_link(&file)? {
                return Err(io::Error::other("it is a link"));
            }
            Ok(file)
        }

        pub(in crate::manifest::build) fn kind(&self, name: &OsStr) -> io::Result<Kind> {
            let found = self.open_rel(name, 0, LOOK)?;
            let meta = found.metadata()?;
            Ok(if is_link(&found)? {
                Kind::Link
            } else if meta.is_dir() {
                Kind::Dir
            } else if meta.is_file() {
                Kind::File
            } else {
                Kind::Other
            })
        }

        /// The names in the folder, listed from its own handle with `FileIdBothDirectoryInfo`, so no
        /// path is resolved and a swap cannot change which folder is listed. The caller opens each
        /// name from the handle.
        pub(in crate::manifest::build) fn names(&self) -> io::Result<Vec<OsString>> {
            // Eight-byte units, so the buffer is aligned for the structure the call writes into it.
            let mut buffer = vec![0u64; 8192];
            let mut names = Vec::new();
            let mut class = FileIdBothDirectoryRestartInfo;
            loop {
                // SAFETY: the buffer holds `buffer.len() * 8` bytes, which the call is told.
                let got = unsafe {
                    GetFileInformationByHandleEx(
                        self.0.as_raw_handle() as HANDLE,
                        class,
                        buffer.as_mut_ptr().cast::<c_void>(),
                        (buffer.len() * 8) as u32,
                    )
                };
                if got == 0 {
                    let error = io::Error::last_os_error();
                    // `ERROR_NO_MORE_FILES` ends the listing.
                    return if error.raw_os_error() == Some(18) {
                        Ok(names)
                    } else {
                        Err(error)
                    };
                }
                class = FileIdBothDirectoryInfo;
                let base = buffer.as_ptr().cast::<u8>();
                let mut at = 0usize;
                loop {
                    // SAFETY: the call filled the buffer with entries chained by `NextEntryOffset`,
                    // each laid out as the structure says, and `at` stays inside the filled bytes.
                    let entry = unsafe { &*base.add(at).cast::<FILE_ID_BOTH_DIR_INFO>() };
                    let name = unsafe {
                        let start = (&raw const entry.FileName).cast::<u16>();
                        std::slice::from_raw_parts(start, entry.FileNameLength as usize / 2)
                    };
                    let name = OsString::from_wide(name);
                    if name != "." && name != ".." {
                        names.push(name);
                    }
                    if entry.NextEntryOffset == 0 {
                        break;
                    }
                    at += entry.NextEntryOffset as usize;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::engine::open_folder;

    /// The names come from the handle, so a folder swapped for another under the same path after it
    /// was opened does not change which folder is listed.
    #[test]
    fn the_names_are_those_of_the_open_folder_and_not_of_whatever_the_path_leads_to_now() {
        let dir = tempfile::tempdir().unwrap();
        let sent = dir.path().join("sent");
        fs::create_dir(&sent).unwrap();
        fs::write(sent.join("inside.txt"), b"a").unwrap();
        fs::create_dir(sent.join("sub")).unwrap();
        let handle = Dir::from_file(open_folder(&sent).unwrap());

        let moved = fs::rename(&sent, dir.path().join("moved"));
        if cfg!(windows) && moved.is_err() {
            // A folder with a handle open cannot be renamed here: the swap itself cannot happen.
            return;
        }
        moved.unwrap();
        fs::create_dir(&sent).unwrap();
        fs::write(sent.join("outside.txt"), b"b").unwrap();

        let mut names = handle.names().unwrap();
        names.sort();
        assert_eq!(names, [OsString::from("inside.txt"), OsString::from("sub")]);
    }

    #[test]
    fn an_empty_folder_lists_nothing_and_a_big_one_lists_everything() {
        let dir = tempfile::tempdir().unwrap();
        let empty = Dir::from_file(open_folder(dir.path()).unwrap());
        assert!(empty.names().unwrap().is_empty());
        for n in 0..3000 {
            fs::write(dir.path().join(format!("file-{n:05}.txt")), b"x").unwrap();
        }
        assert_eq!(empty.names().unwrap().len(), 3000);
    }
}
