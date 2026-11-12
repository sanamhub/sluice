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
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::ptr::{null, null_mut};

    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFinalPathNameByHandleW,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    use super::{Dir, File, Kind, OsStr, OsString, io};
    use crate::engine::is_link;

    /// `FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE`.
    const ACCESS: u32 = 0x1 | 0x80 | 0x10_0000;
    const NOT_FOUND: [i32; 2] = [0xC000_0034_u32 as i32, 0xC000_003A_u32 as i32];

    impl Dir {
        fn open_rel(&self, name: &OsStr, options: u32) -> io::Result<File> {
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
                    ACCESS,
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
            let folder = self.open_rel(name, FILE_DIRECTORY_FILE)?;
            if is_link(&folder)? {
                return Err(io::Error::other("it is a link"));
            }
            Ok(Dir(folder))
        }

        /// The file `name`, which fails when it is not a file or is a link.
        pub(in crate::manifest::build) fn open_file(&self, name: &OsStr) -> io::Result<File> {
            let file = self.open_rel(name, FILE_NON_DIRECTORY_FILE)?;
            if is_link(&file)? {
                return Err(io::Error::other("it is a link"));
            }
            Ok(file)
        }

        pub(in crate::manifest::build) fn kind(&self, name: &OsStr) -> io::Result<Kind> {
            let found = self.open_rel(name, 0)?;
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

        /// The names in the folder. They are listed from the folder's own path as the system gives it
        /// for the handle, and every one is then opened from the handle, so a name that came from
        /// another folder opens nothing here.
        pub(in crate::manifest::build) fn names(&self) -> io::Result<Vec<OsString>> {
            let mut buffer = vec![0u16; 1024];
            let length = loop {
                // SAFETY: the buffer holds `buffer.len()` units and the handle is open.
                let written = unsafe {
                    GetFinalPathNameByHandleW(
                        self.0.as_raw_handle() as HANDLE,
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        0,
                    )
                } as usize;
                if written == 0 {
                    return Err(io::Error::last_os_error());
                }
                if written < buffer.len() {
                    break written;
                }
                buffer.resize(written + 1, 0);
            };
            let path = String::from_utf16(&buffer[..length]).map_err(io::Error::other)?;
            let mut names = Vec::new();
            for entry in std::fs::read_dir(&path)? {
                names.push(entry?.file_name());
            }
            Ok(names)
        }
    }
}
