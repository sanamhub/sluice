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

use super::{ADMINISTRATORS, LocalBlock, SYSTEM, user_sid};

/// Whether the entry at `path` was made by this user, `Administrators` or `SYSTEM`, read from a
/// handle opened without following a link. This is the check for an entry that appeared in a home
/// while it was being made private, and for every folder of the home on every open: a name is not
/// proof that this user made what it holds.
pub(in crate::home) fn is_owned_by_this_user(path: &Path) -> io::Result<bool> {
    let entry = open_entry(path)?;
    is_owned_by_us_handle(&entry)
}

/// Whether the user this process runs as, `Administrators` or `SYSTEM` owns the object `file` is a
/// handle to. The owner is read from that handle, so a path swapped for another object after it was
/// opened changes nothing. `SYSTEM` and `Administrators` can already do anything to the file, so
/// accepting them adds no risk, and a home a backup tool restored still opens.
pub(in crate::home) fn is_owned_by_us_handle(file: &File) -> io::Result<bool> {
    let owner = owner_sid_of(file)?;
    if owner == ADMINISTRATORS || owner == SYSTEM {
        return Ok(true);
    }
    // A token whose user cannot be read is an error, and not "belongs to someone else": the caller
    // would otherwise name the wrong owner.
    Ok(user_sid()? == owner)
}

/// A handle to an entry with the access that reading its owner needs. `BACKUP_SEMANTICS` is what
/// lets a folder be opened at all.
pub(super) fn open_entry(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// What one opened handle to a folder says: whether it is a real folder of this user, and its
/// identity.
pub(in crate::home) enum Checked {
    /// A folder of this user, named by its volume and file index.
    Folder { first: u64, second: u64 },
    /// A link, a junction, another reparse point, or something that is not a folder.
    NotAFolder,
    /// A real folder that another user made.
    SomeoneElses,
}

/// Checks the entry at `path` for its kind and its owner and reads its identity, all from one handle
/// opened without following a link. The three answers describe one object: a name swapped for
/// another between two calls cannot make a check of one folder an identity of another.
pub(in crate::home) fn checked_folder(path: &Path) -> io::Result<Checked> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let (entry, info) = information(path)?;
    let attributes = info.dwFileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Ok(Checked::NotAFolder);
    }
    if !is_owned_by_us_handle(&entry)? {
        return Ok(Checked::SomeoneElses);
    }
    Ok(Checked::Folder {
        first: u64::from(info.dwVolumeSerialNumber),
        second: index(&info),
    })
}

/// The volume number and file index of the entry at `path`, read from a handle opened without
/// following a link. Both numbers come from the file system, not from the path, so a folder renamed
/// away and another put under the same name are told apart. The stable `Metadata` accessors for
/// them are still unstable, which is why this goes through the call.
pub(crate) fn folder_id(path: &Path) -> io::Result<(u64, u64)> {
    let (_entry, info) = information(path)?;
    Ok((u64::from(info.dwVolumeSerialNumber), index(&info)))
}

/// The number of names the entry at `path` has on its volume, read from a handle opened without
/// following a link. A file that two names share can be written through either of them, so the
/// count is what tells a store whose bytes it owns from one that shares them with an output file.
pub(crate) fn link_count(path: &Path) -> io::Result<u32> {
    let (_, info) = information(path)?;
    Ok(info.nNumberOfLinks)
}

/// The handle and the file information of the entry at `path`, read in one open. The handle is kept
/// until the information has been read, so both describe the object the name held at that moment.
fn information(
    path: &Path,
) -> io::Result<(
    File,
    windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION,
)> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let entry = open_entry(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for the whole call and `info` is a valid place for the structure
    // the call fills.
    let ok = unsafe { GetFileInformationByHandle(entry.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((entry, info))
}

fn index(info: &windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION) -> u64 {
    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
}

/// The owner of the object `file` is a handle to, as SID text.
pub(super) fn owner_sid_of(file: &File) -> io::Result<String> {
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
