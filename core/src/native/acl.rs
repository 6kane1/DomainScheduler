//! The Windows part of `StateDir.cs`: reads, judges and fixes the owner and permissions of the state folder.
//! The judgement itself is the pure `StateDir::judge`.

use super::win::{from_wide_ptr, wide};
use crate::base::l::L;
use crate::base::state_dir::{Ace, StateDir, Trust};
use crate::base::time::DateTime;
use crate::lf;
use std::path::Path;
use windows_sys::Win32::Foundation::{LocalFree, BOOL, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, ACL, ACL_SIZE_INFORMATION,
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    SE_DACL_PROTECTED,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, FindClose, FindFirstFileW, GetFileAttributesW, MoveFileExW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    INVALID_FILE_ATTRIBUTES, WIN32_FIND_DATAW,
};

/// SYSTEM and Administrators full control, Users read and execute; inherited by files and folders; protected.
const DACL_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";

fn attributes(p: &Path) -> Option<u32> {
    let w = wide(&p.to_string_lossy());
    let a = unsafe { GetFileAttributesW(w.as_ptr()) };
    (a != INVALID_FILE_ATTRIBUTES).then_some(a)
}

fn dir_exists(p: &Path) -> bool {
    attributes(p).map(|a| a & FILE_ATTRIBUTE_DIRECTORY != 0).unwrap_or(false)
}

fn is_dangling_link(p: &Path) -> bool {
    let w = wide(&p.to_string_lossy());
    // SAFETY: plain data.
    let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let hf = unsafe { FindFirstFileW(w.as_ptr(), &mut fd) };
    if hf == INVALID_HANDLE_VALUE || hf.is_null() {
        return false;
    }
    unsafe { FindClose(hf) };
    fd.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// A security descriptor from SDDL, freed on drop.
struct Sd(PSECURITY_DESCRIPTOR);

impl Sd {
    fn from_sddl(s: &str) -> Result<Sd, String> {
        let w = wide(s);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Sd(sd))
    }
}

impl Drop for Sd {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0 as _) };
        }
    }
}

fn sid_string(sid: *mut core::ffi::c_void) -> Option<String> {
    if sid.is_null() {
        return None;
    }
    let mut p: *mut u16 = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut p) } == 0 {
        return None;
    }
    let s = unsafe { from_wide_ptr(p) };
    unsafe { LocalFree(p as _) };
    Some(s)
}

/// Reads the folder's owner and permission list and judges them.
fn read(p: &Path) -> Result<Trust, String> {
    let w = wide(&p.to_string_lossy());
    let mut owner: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let r = unsafe {
        GetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, std::ptr::null_mut(), &mut dacl, std::ptr::null_mut(), &mut sd)
    };
    if r != 0 {
        return Err(super::reg::error_text(r));
    }
    let _free = Sd(sd);
    let owner_sid = sid_string(owner);
    let mut control: u16 = 0;
    let mut rev: u32 = 0;
    unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut rev) };
    let (mut present, mut defaulted): (BOOL, BOOL) = (0, 0);
    let mut dacl2: *mut ACL = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl2, &mut defaulted) };
    let dacl_present = present != 0 && !dacl2.is_null();
    let mut aces = Vec::new();
    if dacl_present {
        let mut info = ACL_SIZE_INFORMATION { AceCount: 0, AclBytesInUse: 0, AclBytesFree: 0 };
        unsafe { GetAclInformation(dacl2, &mut info as *mut _ as *mut _, std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation) };
        for i in 0..info.AceCount {
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            if unsafe { GetAce(dacl2, i, &mut ace) } == 0 || ace.is_null() {
                continue;
            }
            // SAFETY: GetAce gives a pointer to a whole ACE inside the ACL (header first, AceSize bytes long).
            let (ty, flags, size) = unsafe {
                let b = ace as *const u8;
                (*b, *b.add(1), u16::from_le_bytes([*b.add(2), *b.add(3)]))
            };
            let bytes = unsafe { std::slice::from_raw_parts(ace as *const u8, size as usize) };
            let mask = if bytes.len() >= 8 { i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) } else { 0 };
            // .NET KnownAce: the common types (0-3, 9, 10, 13, 14), compound (4) and object (5-8, 11, 12, 15, 16) ACEs.
            let sid_off = match ty {
                0..=3 | 9 | 10 | 13 | 14 => Some(8usize),
                4 => Some(12),
                5..=8 | 11 | 12 | 15 | 16 => {
                    let of = if bytes.len() >= 12 { u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) } else { 0 };
                    Some(12 + if of & 1 != 0 { 16 } else { 0 } + if of & 2 != 0 { 16 } else { 0 })
                }
                _ => None,
            };
            let Some(off) = sid_off.filter(|o| *o < bytes.len()) else {
                // object/callback entries we cannot read: not ours
                aces.push(Ace::new("?", StateDir::WRITE_MASK, true, 0, false));
                continue;
            };
            let sid = sid_string(unsafe { (ace as *mut u8).add(off) } as *mut core::ffi::c_void).unwrap_or_else(|| "?".into());
            let allow = matches!(ty, 0 | 5 | 9 | 11);
            aces.push(Ace::new(&sid, mask, allow, flags as i32, flags & 0x10 != 0));
        }
    }
    let prot = control & SE_DACL_PROTECTED != 0;
    let _ = dacl;
    Ok(StateDir::judge(owner_sid.as_deref(), dacl_present, prot, Some(&aces)))
}

/// Replaces only the permission list (protected from inheritance); the owner stays.
fn reset_dacl(p: &Path) -> Result<(), String> {
    let sd = Sd::from_sddl(DACL_SDDL)?;
    let (mut present, mut defaulted): (BOOL, BOOL) = (0, 0);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted) };
    let w = wide(&p.to_string_lossy());
    let r = unsafe {
        SetNamedSecurityInfoW(w.as_ptr() as *mut u16, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), dacl, std::ptr::null())
    };
    if r != 0 {
        return Err(super::reg::error_text(r));
    }
    Ok(())
}

fn create_with(p: &Path, sddl: &str) -> Result<(), String> {
    let sd = Sd::from_sddl(sddl)?;
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: 0 };
    let w = wide(&p.to_string_lossy());
    if unsafe { CreateDirectoryW(w.as_ptr(), &sa) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// A folder somebody else could write to: renamed (never read), so a fresh one can be made.
fn move_away(p: &Path, notes: &mut Vec<String>) -> Result<(), String> {
    let to = format!("{}.untrusted-{}", p.display(), DateTime::utc_now().format("yyyyMMdd-HHmmss", &crate::base::fmt::Culture::invariant()));
    // A link is moved as the link itself, its target is not touched.
    let a = wide(&p.to_string_lossy());
    let b = wide(&to);
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    notes.push(lf!("State folder: {0} could be changed by other accounts; it was renamed to {1} and is not used", p.display().to_string(), to));
    Ok(())
}

/// `StateDir.Secure(p)`.
pub fn secure(p: &Path, notes: &mut Vec<String>) -> Result<(), String> {
    let mut attrs = attributes(p);
    if attrs.is_none() && is_dangling_link(p) {
        attrs = Some(FILE_ATTRIBUTE_REPARSE_POINT);
    }
    if let Some(a) = attrs {
        let link = a & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        let dir = a & FILE_ATTRIBUTE_DIRECTORY != 0;
        // Its permissions cannot even be read: somebody shut administrators out.
        let t = if link || !dir { Trust::Untrusted } else { read(p).unwrap_or(Trust::Untrusted) };
        if t == Trust::ResetPermissions {
            // The owner is already SYSTEM / Administrators / TrustedInstaller here: only the list is replaced.
            reset_dacl(p)?;
            notes.push(L::t("State folder: its permissions were reset (only administrators can change it)").to_string());
        } else if t == Trust::Untrusted {
            move_away(p, notes)?;
        }
    }
    if !dir_exists(p) {
        // Created with its permissions in the same call. Owner: the Administrators group (an elevated token's
        // default anyway); if Windows refuses to set it, the token's default owner is checked below.
        if let Err(e) = create_with(p, &format!("O:BA{}", DACL_SDDL)) {
            if dir_exists(p) {
                return Err(e);
            }
            create_with(p, DACL_SDDL)?;
        }
    }
    // Whatever Windows made of the list: nobody but SYSTEM / Administrators / TrustedInstaller may change anything.
    let a = attributes(p).ok_or_else(|| std::io::Error::last_os_error().to_string())?;
    if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 || read(p)? == Trust::Untrusted {
        return Err(L::t("its owner or permissions are not as they must be").to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder gets the protected permissions; a second check trusts it (Windows or Wine).
    #[test]
    fn a_new_folder_is_created_protected_and_then_trusted() {
        if crate::native::sys::is_wine() {
            return; // Wine keeps its own default owner and permissions: only real Windows can be checked
        }
        let p = std::env::temp_dir().join(format!("ds-acl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let mut notes = Vec::new();
        let r = secure(&p, &mut notes);
        assert!(r.is_ok(), "{:?}", r);
        assert!(p.is_dir());
        assert_eq!(read(&p).unwrap(), Trust::Trusted);
        assert!(secure(&p, &mut notes).is_ok());
        assert!(notes.is_empty(), "{:?}", notes);
        let _ = std::fs::remove_dir_all(&p);
    }
}
