//! A private access list for what sluice creates on Windows.
//!
//! A new file or folder on Windows inherits the access list of the folder above it. That list is
//! the user's own on a stock profile, but nothing makes it so: on the machine this was written on,
//! `%LOCALAPPDATA%` lets a local group read everything under it. The identity key must not depend
//! on that, so this module sets a list of its own and stops inheritance: the current user, `SYSTEM`
//! and `Administrators`, each with full control. That is what a stock profile gives.
//!
//! The user is the one in the process token (`TokenUser`), not Owner Rights or Creator Owner. A
//! process that runs elevated makes `Administrators` the owner of what it creates, and a later
//! process that does not would then be locked out of its own key.
//!
//! The list is built as an SDDL string and applied through the handle of the file or folder, not
//! its path. The string is readable, `SYSTEM` and `Administrators` have two-letter aliases in it
//! that need no buffers, and the same form reads a list back in the tests. A handle names the
//! object that was created, so a path swapped for another object between the create and the call
//! changes nothing. The handle needs `WRITE_DAC`, which the callers ask for when they open it.
//!
//! The text Windows gives back is not always the text it was given: the built-in Administrator
//! (the account with RID 500) reads back as `LA`, and `SYSTEM` as `SY`. A list that was set with
//! the user's full SID would then look different from the one that is there. Both texts go through
//! the same conversion before they are compared, so a list that is right is not refused for the
//! spelling of a trustee.
//!
//! This is the only module in the crate with `unsafe`. Every block says why its pointers are valid.

#![deny(clippy::undocumented_unsafe_blocks)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

mod links;
mod lists;
mod owner;
mod space;

#[cfg(test)]
mod admin;

pub(in crate::home) use links::{links_of, links_of_path};
pub(in crate::home) use lists::{is_private_dir, is_private_file};
pub(crate) use owner::{folder_id, link_count};
pub(in crate::home) use owner::{is_owned_by_this_user, is_owned_by_us_handle};
pub(crate) use space::free_bytes;

/// Object and container inherit: a folder's entries reach the files and folders made inside it.
const INHERIT: &str = "OICI";

/// Stops inheritance on a folder and gives it the private list, with entries that what is created
/// inside it inherits. Opens the folder itself, so the caller passes a path.
#[cfg(test)]
pub(super) fn restrict_dir(path: &Path) -> io::Result<()> {
    let dir = open_dir(path)?;
    set_dacl(&dir, &private_sddl(&user_sid()?, INHERIT))
}

/// Stops inheritance on a file and gives it the private list. The handle must have been opened
/// with `WRITE_DAC`.
pub(super) fn restrict_file(file: &File) -> io::Result<()> {
    set_dacl(file, &private_sddl(&user_sid()?, ""))
}

/// [`restrict_file`] for a folder: the list carries the inherit flags, so what is made inside it
/// gets the same three entries. It is the one call that sets a home's list through the handle the
/// home was opened with, so the owner that was checked and the object that is restricted are the
/// same.
pub(in crate::home) fn restrict_dir_file(file: &File) -> io::Result<()> {
    set_dacl(file, &private_sddl(&user_sid()?, INHERIT))
}

/// Whether the folder `file` is a handle to has the private list, read back through that handle.
pub(in crate::home) fn is_private_dir_file(file: &File) -> io::Result<bool> {
    lists::has_private_list_text(&dacl_sddl_of(file)?, INHERIT)
}

/// A handle to a folder with the access that changing its list needs: `WRITE_DAC`, and
/// `READ_CONTROL` because `SetSecurityInfo` reads the list it replaces and answers "Access is
/// denied" without it. `BACKUP_SEMANTICS` is what lets Windows open a folder at all.
/// `OPEN_REPARSE_POINT` means a junction put in the folder's place is changed itself and not
/// followed to wherever it points. The default share mode stays, and the access is named and not
/// `MAXIMUM_ALLOWED`, because with either changed `SetSecurityInfo` stops pushing the inheritable
/// entries into children that already exist.
pub(in crate::home) fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// The list as SDDL: allow full control to the user, `SY` (SYSTEM) and `BA` (Administrators).
fn private_sddl(user: &str, inherit: &str) -> String {
    format!("D:(A;{inherit};FA;;;{user})(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)")
}

/// Memory the system allocated with `LocalAlloc` and handed over. It is freed when the guard
/// drops, so an early return with `?` cannot leak it.
struct LocalBlock(*mut c_void);

impl LocalBlock {
    /// Wraps the pointer a successful call returned. A null one is an error, not a block.
    fn new(pointer: *mut c_void) -> io::Result<Self> {
        if pointer.is_null() {
            return Err(io::Error::other("the system returned no data"));
        }
        Ok(Self(pointer))
    }
}

impl Drop for LocalBlock {
    fn drop(&mut self) {
        // SAFETY: `new` only makes a guard from a non-null pointer that a Windows call allocated
        // with LocalAlloc and documents as freed with LocalFree. The guard is not Clone and has no
        // other way out, so this is the only free.
        unsafe { LocalFree(self.0) };
    }
}

/// The descriptor of `sddl`, allocated by the system and freed when the guard drops.
fn descriptor_of(sddl: &str) -> io::Result<LocalBlock> {
    let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: `wide` is NUL terminated and outlives the call. `descriptor` is a valid place for
    // the out pointer. The size out parameter is optional and null.
    let made = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if made == 0 {
        return Err(io::Error::last_os_error());
    }
    LocalBlock::new(descriptor)
}

/// The text of `descriptor`, with only the access list written.
fn sddl_of(descriptor: &LocalBlock) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: `descriptor` is a descriptor the system made and the guard keeps alive. `text` is a
    // valid place for the string pointer, and the length out parameter is null.
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let text = LocalBlock::new(text.cast())?;
    // SAFETY: on success `text` is a NUL terminated UTF-16 string in a LocalAlloc block that the
    // guard keeps alive for the call.
    unsafe { wide_to_string(text.0.cast()) }
}

/// `sddl` as Windows writes the same list back. Windows shortens a trustee to its two-letter alias
/// when the system has one: the built-in Administrator (the account with RID 500) becomes `LA`,
/// `SYSTEM` becomes `SY`, and `Administrators` becomes `BA`. A list built from full SIDs and a list
/// read back with aliases name the same access, but only after both have been through this
/// conversion.
pub(super) fn canonical_sddl(sddl: &str) -> io::Result<String> {
    sddl_of(&descriptor_of(sddl)?)
}

/// Makes the descriptor of `sddl` and sets its DACL on the object `file` is a handle to, with
/// inheritance from the parent stopped.
fn set_dacl(file: &File, sddl: &str) -> io::Result<()> {
    let descriptor = descriptor_of(sddl)?;
    let (mut present, mut acl, mut defaulted) = (0, ptr::null_mut(), 0);
    // SAFETY: `descriptor` is the valid descriptor made above and stays alive to the end of the
    // function. The three out parameters are valid places for a BOOL, a pointer and a BOOL.
    let found =
        unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted) };
    if found == 0 {
        return Err(io::Error::last_os_error());
    }
    if present == 0 || acl.is_null() {
        return Err(io::Error::other("the access list has no entries"));
    }

    // SAFETY: the handle is open for as long as the borrow of `file` lasts, which covers the call.
    // `acl` points into the memory of `descriptor`, which is not freed until this function
    // returns. The owner, group and SACL pointers are null because the flags ask for the DACL only.
    let status = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

/// The SID of the user this process runs as, as text such as `S-1-5-21-...`.
fn user_sid() -> io::Result<String> {
    let token = open_token()?;
    let mut needed = 0u32;
    // SAFETY: the token handle is open. A null buffer with length 0 is how the call is asked for
    // the size it needs, and `needed` is a valid place for it.
    let sized = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    let size_error = io::Error::last_os_error();
    if sized != 0 || size_error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(size_error);
    }

    // A u64 buffer is 8-aligned, which TOKEN_USER (it holds a pointer) needs.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the token handle is open. `buffer` has at least `needed` bytes, the length the
    // previous call asked for, and it is passed with that length.
    let filled = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if filled == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so the start of `buffer` holds a TOKEN_USER that it wrote. The
    // buffer is 8-aligned and larger than that struct.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: `sid` points to the SID inside `buffer`, which is alive until the end of this
    // function and so for the whole call.
    unsafe { sid_to_string(sid) }
}

/// The access token of this process, closed when the returned handle drops.
fn open_token() -> io::Result<OwnedHandle> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo handle that is always valid for this process.
    // `token` is a valid place for the handle the call writes.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `token` is a new open handle that nothing else owns.
    // OwnedHandle closes it once, on drop.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

/// # Safety
///
/// `sid` must point to a valid SID that stays valid for the call.
unsafe fn sid_to_string(sid: *mut c_void) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: the caller promises `sid` is a valid SID. `text` is a valid place for the string
    // pointer the call writes.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let text = LocalBlock::new(text.cast())?;
    // SAFETY: on success `text` is a NUL terminated UTF-16 string in a LocalAlloc block, which
    // the guard keeps alive for the call.
    unsafe { wide_to_string(text.0.cast()) }
}

/// Copies a NUL terminated UTF-16 string out of memory the system owns.
///
/// # Safety
///
/// `text` must point to UTF-16 units that are readable up to and including a terminating zero.
unsafe fn wide_to_string(text: *const u16) -> io::Result<String> {
    let mut len = 0;
    // SAFETY: the caller promises every unit up to the zero is readable, and the loop stops there.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: the `len` units before the zero were just read, so they are readable and set.
    let units = unsafe { std::slice::from_raw_parts(text, len) };
    String::from_utf16(units).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The access list of `path` as SDDL, for `sluice doctor` and the tests to check what was set.
/// Reads by path, which is a different route from the one that set it.
fn dacl_sddl(path: &Path) -> io::Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: `wide` is NUL terminated and outlives the call. `descriptor` is a valid place for
    // the pointer the call writes. Every other out parameter is optional and null, because only
    // the descriptor is asked for.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    sddl_of(&LocalBlock::new(descriptor)?)
}

/// The access list of the object `file` is a handle to, as SDDL. The handle is the one the list
/// was set through, so the read-back cannot be of another object behind a swapped link.
fn dacl_sddl_of(file: &File) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::GetSecurityInfo;

    let mut descriptor = ptr::null_mut();
    // SAFETY: the handle is open for as long as the borrow of `file` lasts, which covers the call.
    // `descriptor` is a valid place for the pointer the call writes. Every other out parameter is
    // optional and null, because only the DACL is asked for.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    sddl_of(&LocalBlock::new(descriptor)?)
}

/// The well-known SID of `Administrators`, which owns what a process that runs elevated creates.
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// The well-known SID of `SYSTEM`, which owns what the system itself makes and what some backup
/// tools restore.
const SYSTEM: &str = "S-1-5-18";

#[cfg(test)]
mod tests;
