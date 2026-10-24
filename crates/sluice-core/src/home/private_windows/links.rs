//! How many names an object has, from the handle it was opened through.
//!
//! A hard link is a second name for the same file, with the same owner, so the owner check cannot
//! see it. Whoever can write through the other name can change the identity key, so the count
//! decides. `std`'s `number_of_links` is not stable, and `GetFileInformationByHandle` is the call
//! it wraps.

use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

use super::owner::open_entry;

/// The number of names the object `file` is a handle to has, which the key checks compare with one.
pub(in crate::home) fn links_of(file: &File) -> io::Result<u64> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for as long as the borrow of `file` lasts, which covers the call,
    // and `info` is a valid place for the structure the call writes.
    let done = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if done == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(info.nNumberOfLinks))
}

/// The number of names the entry at `path` has, read through a handle that does not follow a link,
/// for a caller that only has a path, such as `sluice doctor`.
pub(in crate::home) fn links_of_path(path: &Path) -> io::Result<u64> {
    links_of(&open_entry(path)?)
}
