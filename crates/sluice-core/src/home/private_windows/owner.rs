//! Who made an object: the owner SID, read through a handle that did not follow a link.
//!
//! The owner is read by handle and not by path because a link put at the name between a check and
//! the operation that follows it would move both to wherever it points. `OPEN_REPARSE_POINT` makes
//! the handle name the link itself, and the caller refuses a link before it uses the answer.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL,
};

use super::{ADMINISTRATORS, LocalBlock, user_sid};

/// Whether the entry at `path` was made by this user or `Administrators`, read from a handle
/// opened without following a link. This is the check for an entry that appeared in a home while
/// it was being made private: a name is not proof that this user made what it holds.
pub(in crate::home) fn is_owned_by_this_user(path: &Path) -> io::Result<bool> {
    let entry = open_entry(path)?;
    is_owned_by_us_handle(&entry)
}

/// Whether the owner of `path` is the user this process runs as, or `Administrators`. An owner can
/// always rewrite the access list, whatever the list says, so a home that another user made first
/// would stay theirs to open up again after sluice set it private. Read through a handle that does
/// not follow a link.
pub(in crate::home) fn is_owned_by_us(path: &Path) -> io::Result<bool> {
    is_owned_by_this_user(path)
}

/// Whether the user this process runs as, or `Administrators`, owns the object `file` is a handle
/// to. It is the same answer as [`super::is_owned_by_us`], without a second open by path.
pub(in crate::home) fn is_owned_by_us_handle(file: &File) -> io::Result<bool> {
    owner_sid_of(file)
        .map(|owner| owner == ADMINISTRATORS || user_sid().is_ok_and(|us| us == owner))
}

/// A handle to an entry with the access that reading its owner needs. `BACKUP_SEMANTICS` is what
/// lets a folder be opened at all.
fn open_entry(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// The owner of the object `file` is a handle to, as SID text.
fn owner_sid_of(file: &File) -> io::Result<String> {
    let (mut owner, mut descriptor) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: the handle is open for as long as the borrow of `file` lasts, which covers the call.
    // `owner` and `descriptor` are valid places for the pointers the call writes. The group, DACL
    // and SACL out parameters are optional and null, because only the owner is asked for.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // Held to the end of the function: `owner` points into it.
    let _descriptor = LocalBlock::new(descriptor)?;
    if owner.is_null() {
        return Err(io::Error::other("the object has no owner"));
    }
    // SAFETY: on success `owner` points to the owner SID inside the descriptor, which
    // `_descriptor` keeps alive until the function returns, after this call.
    unsafe { super::sid_to_string(owner) }
}
