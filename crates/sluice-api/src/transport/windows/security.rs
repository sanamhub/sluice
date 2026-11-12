//! The platform calls behind the named pipe: who owns a process, the access list the pipe is made
//! with, and who is at the other end. Everything that needs `unsafe` in the pipe is in this file.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, PSID, RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeServerProcessId, ImpersonateNamedPipeClient};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// A handle that closes when it drops.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this module and is closed once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// The security identifier of the user of the token of `process`, as the bytes of the SID.
fn process_token_sid(process: HANDLE) -> io::Result<Vec<u8>> {
    let mut token: HANDLE = null_mut();
    // SAFETY: `process` is a valid process handle and `token` is a place for one handle.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    token_user_sid(&Owned(token))
}

/// The user of an open token, as the bytes of the SID.
fn token_user_sid(token: &Owned) -> io::Result<Vec<u8>> {
    let mut needed = 0u32;
    // SAFETY: a call with no buffer only reports the size that is needed.
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    // Eight-byte units, so the buffer is aligned for the structure the call writes into it.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer holds `needed` bytes and the call writes at most that many.
    let read = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call filled the buffer with a TOKEN_USER whose SID points inside the buffer.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    Ok(sid_bytes(sid))
}

/// The bytes of a SID that is valid for the length the system reports for it.
fn sid_bytes(sid: PSID) -> Vec<u8> {
    // SAFETY: the SID is valid for as long as its owner, which the caller keeps alive.
    let length = unsafe { GetLengthSid(sid) } as usize;
    // SAFETY: `GetLengthSid` is the size of the structure at `sid`.
    unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) }.to_vec()
}

/// The user this process runs as.
pub(super) fn own_sid() -> io::Result<Vec<u8>> {
    // SAFETY: the pseudo handle of the current process needs no closing.
    process_token_sid(unsafe { GetCurrentProcess() })
}

/// The user a process with this id runs as. A process this user cannot query is an error, and an
/// error refuses the peer.
fn process_sid(pid: u32) -> io::Result<Vec<u8>> {
    // SAFETY: opens a handle for query access only; the result is checked and closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Owned(process);
    process_token_sid(process.0)
}

/// The text of a SID, such as `S-1-5-21-...`.
pub(super) fn sid_text(sid: &[u8]) -> io::Result<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY: `sid` is the bytes of a SID and `text` is a place for the pointer the call allocates.
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast::<c_void>(), &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call returned a NUL-terminated wide string, which is freed once, below.
    let result = unsafe {
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, length))
    };
    // SAFETY: the string was allocated by the call with `LocalAlloc`.
    unsafe { LocalFree(text.cast::<c_void>()) };
    result.map_err(io::Error::other)
}

/// The access list a pipe is made with: full access for this user, `SYSTEM` and `Administrators`,
/// and nobody else. `P` stops it inheriting anything.
pub(super) fn private_sddl(own: &[u8]) -> io::Result<String> {
    Ok(format!(
        "D:P(A;;GA;;;{})(A;;GA;;;SY)(A;;GA;;;BA)",
        sid_text(own)?
    ))
}

/// Makes one instance of the pipe with the access list `sddl`. The list is built, used and freed
/// inside this call, so no pointer to it outlives it.
pub(super) fn create(
    options: &ServerOptions,
    name: &str,
    sddl: &str,
) -> io::Result<NamedPipeServer> {
    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: *mut c_void = null_mut();
    // SAFETY: `wide` is NUL-terminated and `descriptor` is a place for the pointer it allocates.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: `attributes` and the descriptor it points to stay alive until after the call.
    let made = unsafe {
        options.create_with_security_attributes_raw(name, (&raw mut attributes).cast::<c_void>())
    };
    // SAFETY: the descriptor was allocated by the conversion with `LocalAlloc`.
    unsafe { LocalFree(descriptor) };
    made
}

/// Puts the thread back as it was when it drops, so an impersonation never outlives the call that
/// made it, on any way out.
struct Reverts;

impl Drop for Reverts {
    fn drop(&mut self) {
        // SAFETY: ends an impersonation of the calling thread, which is all this call does.
        unsafe {
            RevertToSelf();
        }
    }
}

/// The user at the client end of `server`, which the system tells by letting this thread act as
/// the client for the length of the call: the thread's token is the client's, and is read and
/// dropped before the thread reverts. Nothing here waits, so the call stays on one thread.
pub(super) fn client_sid(server: &NamedPipeServer) -> io::Result<Vec<u8>> {
    // SAFETY: the handle is open for as long as `server` is borrowed.
    if unsafe { ImpersonateNamedPipeClient(server.as_raw_handle() as HANDLE) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _revert = Reverts;
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo handle of the thread needs no closing, and `token` is a place for one
    // handle. The token is opened as the process, so the open does not depend on the client's rights.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    token_user_sid(&Owned(token))
}

/// The user of the process that made the pipe a client has opened.
pub(super) fn server_sid(client: &impl AsRawHandle) -> io::Result<Vec<u8>> {
    let mut pid = 0u32;
    // SAFETY: the handle is open for as long as `client` is borrowed.
    let got = unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut pid) };
    if got == 0 {
        return Err(io::Error::last_os_error());
    }
    process_sid(pid)
}

/// The users the access list of `server` allows, as SID text, read back from the pipe's own handle.
#[cfg(test)]
pub(super) fn allowed_sids(server: &NamedPipeServer) -> io::Result<Vec<String>> {
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, GetAce, GetAclInformation,
    };
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: *mut c_void = null_mut();
    // SAFETY: the handle is open for as long as `server` is borrowed, and the two out parameters
    // are places for pointers; the descriptor is freed below.
    let got = unsafe {
        GetSecurityInfo(
            server.as_raw_handle() as HANDLE,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if got != 0 {
        return Err(io::Error::from_raw_os_error(got as i32));
    }
    let mut size: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `dacl` came from the call above and `size` is the structure the class names.
    let ok = unsafe {
        GetAclInformation(
            dacl,
            (&raw mut size).cast::<c_void>(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    let mut sids = Vec::new();
    for index in 0..if ok == 0 { 0 } else { size.AceCount } {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: the index is below the count the ACL reported.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            continue;
        }
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: an allowed ace holds its SID from `SidStart`, which is how the type lays it out.
        let sid = unsafe { (&raw mut (*ace).SidStart).cast::<c_void>() };
        sids.push(sid_text(&sid_bytes(sid))?);
    }
    // SAFETY: the descriptor was allocated by `GetSecurityInfo` with `LocalAlloc`.
    unsafe { LocalFree(descriptor) };
    Ok(sids)
}
