//! The machine's own built-in Administrator, which Windows writes as `LA`.
//!
//! The SID is the machine's account domain with RID 500, and not the current account's domain with
//! RID 500: on a machine joined to another domain, or signed in with a Microsoft Entra account,
//! those are two different trustees. The account domain comes from the local security authority,
//! so the answer does not depend on which account runs the test.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr;

use windows_sys::Win32::Security::Authentication::Identity::{
    LSA_HANDLE, LSA_OBJECT_ATTRIBUTES, LsaClose, LsaFreeMemory, LsaOpenPolicy,
    LsaQueryInformationPolicy, POLICY_ACCOUNT_DOMAIN_INFO, POLICY_VIEW_LOCAL_INFORMATION,
    PolicyAccountDomainInformation,
};
use windows_sys::Win32::Security::{CreateWellKnownSid, WinAccountAdministratorSid};

/// The largest a SID can be, as `SECURITY_MAX_SID_SIZE` gives it, in bytes. Rounded to a `u32`
/// so the buffer is aligned for the SID the call writes.
const MAX_SID_WORDS: usize = 17;

/// The SID of the machine's built-in Administrator, as text such as `S-1-5-21-...-500`.
pub(super) fn built_in_administrator_sid() -> io::Result<String> {
    let policy = open_policy()?;
    let buffer = account_domain(policy.0)?;
    // SAFETY: on success LSA wrote a POLICY_ACCOUNT_DOMAIN_INFO at the start of the block, and the
    // guard keeps the block alive. Its `DomainSid` points inside that block.
    let domain = unsafe { (*buffer.0.cast::<POLICY_ACCOUNT_DOMAIN_INFO>()).DomainSid };
    administrator_of(domain)
}

/// The machine's local policy, closed when the guard drops.
struct Policy(LSA_HANDLE);

impl Drop for Policy {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a handle LsaOpenPolicy returned, and this is its only close.
        let _ = unsafe { LsaClose(self.0) };
    }
}

/// A block LSA allocated for the answer, freed when the guard drops.
struct LsaBlock(*mut c_void);

impl Drop for LsaBlock {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the block LsaQueryInformationPolicy allocated, and this is its only
        // free. The block is not read after the guard drops.
        let _ = unsafe { LsaFreeMemory(self.0) };
    }
}

fn open_policy() -> io::Result<Policy> {
    let mut attributes = LSA_OBJECT_ATTRIBUTES::default();
    attributes.Length = size_of::<LSA_OBJECT_ATTRIBUTES>() as u32;
    let mut policy: LSA_HANDLE = 0;
    // SAFETY: a null system name asks for the local machine's policy, `attributes` is a zeroed
    // structure with its length set, and `policy` is a valid place for the handle.
    let status = unsafe {
        LsaOpenPolicy(
            ptr::null(),
            &attributes,
            POLICY_VIEW_LOCAL_INFORMATION as u32,
            &mut policy,
        )
    };
    if status != 0 {
        return Err(io::Error::other(format!(
            "cannot open the machine's policy: LSA status {status:#x}"
        )));
    }
    Ok(Policy(policy))
}

fn account_domain(policy: LSA_HANDLE) -> io::Result<LsaBlock> {
    let mut buffer = ptr::null_mut();
    // SAFETY: `policy` is open for the call. `buffer` is a valid place for the pointer to the
    // block LSA allocates, which the guard frees.
    let status =
        unsafe { LsaQueryInformationPolicy(policy, PolicyAccountDomainInformation, &mut buffer) };
    if status != 0 {
        return Err(io::Error::other(format!(
            "cannot read the machine's account domain: LSA status {status:#x}"
        )));
    }
    Ok(LsaBlock(buffer))
}

/// The built-in Administrator of account domain `domain`, as its SID text.
fn administrator_of(domain: *mut c_void) -> io::Result<String> {
    let mut sid = [0u32; MAX_SID_WORDS];
    let mut len = (MAX_SID_WORDS * size_of::<u32>()) as u32;
    // SAFETY: `domain` is the domain SID inside the block the caller still holds. `sid` is a
    // buffer of `len` bytes, which is the largest a SID can be, and it is aligned for one.
    let made = unsafe {
        CreateWellKnownSid(
            WinAccountAdministratorSid,
            domain,
            sid.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if made == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `sid` holds the SID the call wrote and stays alive for the call.
    unsafe { super::sid_to_string(sid.as_mut_ptr().cast()) }
}
