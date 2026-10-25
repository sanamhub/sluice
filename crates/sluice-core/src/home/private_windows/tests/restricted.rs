//! A home whose list cannot be changed, on a thread that holds no privilege able to override it.
//!
//! An elevated token holds the restore right, and with `FILE_FLAG_BACKUP_SEMANTICS` that right lets
//! it ask for `WRITE_DAC` whatever the list says. No list can build the refusal under such a token,
//! so the list case above skips there. This case removes the privileges from a copy of the token
//! and puts the thread under it, so the same list refuses the open on every machine.

use std::fs;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{HANDLE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, DuplicateTokenEx,
    LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, RevertToSelf, SE_PRIVILEGE_REMOVED,
    SecurityImpersonation, TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenImpersonation,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, SetThreadToken};

use super::{dacl_sddl, open_dir, set_dacl, user_sid};
use crate::{Error, Home};

/// The access a duplicate asks for, so the thread token can be used for its checks.
const MAXIMUM_ALLOWED: u32 = 0x0200_0000;

/// The privileges that let a token ask for `WRITE_DAC` whatever the list says when the handle is
/// opened with backup semantics.
const BYPASS_PRIVILEGES: [&str; 3] = [
    "SeRestorePrivilege",
    "SeBackupPrivilege",
    "SeTakeOwnershipPrivilege",
];

/// A copy of this process's token with every privilege disabled and removed, put on this thread
/// until the guard drops.
struct WithoutPrivileges {
    _token: OwnedHandle,
}

impl WithoutPrivileges {
    fn new() -> io::Result<Self> {
        let process = process_token()?;
        let token = restricted_impersonation(process.as_raw_handle())?;
        remove_bypass_privileges(token.as_raw_handle())?;
        // SAFETY: a null thread asks for this thread, and `token` is an impersonation token that
        // stays open in the guard. Reverting in `Drop` ends the impersonation.
        if unsafe { SetThreadToken(ptr::null(), token.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { _token: token })
    }
}

/// This process's token, closed when the guard drops.
fn process_token() -> io::Result<OwnedHandle> {
    let mut process: HANDLE = ptr::null_mut();
    // SAFETY: the current process pseudo handle is always valid, and `process` is a valid place
    // for the new handle, which the guard closes.
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `process` is a new open handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(process) })
}

/// A restricted copy of `process` as an impersonation token, with every privilege disabled.
fn restricted_impersonation(process: HANDLE) -> io::Result<OwnedHandle> {
    let mut restricted: HANDLE = ptr::null_mut();
    // SAFETY: `process` is open for the call, every count is zero with a null list, and
    // `restricted` is a valid place for the new handle.
    if unsafe {
        CreateRestrictedToken(
            process,
            DISABLE_MAX_PRIVILEGE,
            0,
            ptr::null(),
            0,
            ptr::null(),
            0,
            ptr::null(),
            &mut restricted,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `restricted` is a token handle the call above made and nothing else owns, so the
    // guard closes it once.
    let restricted = unsafe { OwnedHandle::from_raw_handle(restricted) };
    let mut impersonation: HANDLE = ptr::null_mut();
    // SAFETY: `restricted` is an open token. The attributes are null with the default, and
    // `impersonation` is a valid place for the duplicate.
    if unsafe {
        DuplicateTokenEx(
            restricted.as_raw_handle(),
            MAXIMUM_ALLOWED,
            ptr::null(),
            SecurityImpersonation,
            TokenImpersonation,
            &mut impersonation,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so `impersonation` is a new open handle nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(impersonation) })
}

/// Removes the three privileges that can override an access list from the open token `token`.
fn remove_bypass_privileges(token: HANDLE) -> io::Result<()> {
    for name in BYPASS_PRIVILEGES {
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL terminated and outlives the call, and `luid` is a valid place for
        // the local identifier of the named privilege.
        if unsafe { LookupPrivilegeValueW(ptr::null(), wide.as_ptr(), &mut luid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let state = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_REMOVED,
            }],
        };
        // SAFETY: `token` is the open duplicate for the call. `state` holds one privilege and stays
        // alive for the call; the previous state is not asked for. A privilege the token did not
        // hold is not an error: the function reports that through the last error and returns true.
        if unsafe { AdjustTokenPrivileges(token, 0, &state, 0, ptr::null_mut(), ptr::null_mut()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

impl Drop for WithoutPrivileges {
    fn drop(&mut self) {
        // SAFETY: this thread is under the token this guard set, and this ends the impersonation
        // before `token` closes.
        let _ = unsafe { RevertToSelf() };
    }
}

#[test]
fn a_home_whose_list_cannot_be_changed_is_a_storage_error_without_override_privileges() {
    let dir = tempfile::tempdir().unwrap();
    let user = user_sid().unwrap();
    let sddl = format!("D:P(D;;WD;;;OW)(D;;WD;;;{user})(A;OICI;FA;;;{user})");
    set_dacl(&open_dir(dir.path()).unwrap(), &sddl).unwrap();
    let before = dacl_sddl(dir.path()).unwrap();
    let Ok(without) = WithoutPrivileges::new() else {
        eprintln!("SKIPPED: this process cannot put its thread under a token without privileges");
        return;
    };
    let result = Home::open(dir.path());
    drop(without);
    let Err(Error::Storage(message)) = result else {
        panic!("expected a storage error");
    };
    assert!(message.contains("restrict access to"), "{message}");
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        0,
        "nothing was written"
    );
    assert_eq!(
        dacl_sddl(dir.path()).unwrap(),
        before,
        "the list is as it was"
    );
}
