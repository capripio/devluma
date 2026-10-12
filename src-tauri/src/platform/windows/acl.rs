//! Restrictive file ACLs for private keys (ADR 0010): a protected DACL with a single
//! full-control entry for the current user — no inherited entries, no groups.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAclInformation,
    GetTokenInformation, NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

/// `GENERIC_ALL` access right.
const GENERIC_ALL: u32 = 0x1000_0000;

fn err(e: windows::core::Error) -> std::io::Error {
    std::io::Error::other(e.message())
}

/// The current user's SID, as the raw `TOKEN_USER` buffer (the SID points into it).
fn current_user_token() -> std::io::Result<Vec<u8>> {
    // SAFETY: standard token query; the handle is closed before returning.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(err)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let r = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut _),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        r.map_err(err)?;
        Ok(buf)
    }
}

/// Replaces `path`'s DACL with one entry: full control for the current user.
pub fn restrict_to_current_user(path: &Path) -> std::io::Result<()> {
    let token = current_user_token()?;
    // SAFETY: `token` holds a TOKEN_USER whose SID pointer stays valid while `token` lives.
    let sid: PSID = unsafe { (*(token.as_ptr() as *const TOKEN_USER)).User.Sid };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(sid.0 as *mut u16),
        },
    };
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `access` and `wide` outlive the calls; the new ACL is freed with LocalFree.
    unsafe {
        let mut acl: *mut ACL = std::ptr::null_mut();
        let rc = SetEntriesInAclW(Some(&[access]), None, &mut acl);
        if rc.is_err() {
            return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
        }
        let rc = SetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl),
            None,
        );
        let _ = LocalFree(Some(HLOCAL(acl as *mut _)));
        if rc.is_err() {
            return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
        }
    }
    Ok(())
}

/// Number of entries in `path`'s DACL (for verification and tests).
pub fn dacl_entry_count(path: &Path) -> std::io::Result<u32> {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: out-params are valid; the descriptor is freed with LocalFree.
    unsafe {
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let rc = GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        );
        if rc.is_err() {
            return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
        }
        let mut info = ACL_SIZE_INFORMATION::default();
        let r = GetAclInformation(
            dacl,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        r.map_err(err)?;
        Ok(info.AceCount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_exactly_one_entry_and_keeps_the_file_readable() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("key.pem");
        std::fs::write(&key, b"secret").unwrap();
        assert!(
            dacl_entry_count(&key).unwrap() > 1,
            "temp files inherit several entries"
        );
        restrict_to_current_user(&key).unwrap();
        assert_eq!(dacl_entry_count(&key).unwrap(), 1);
        assert_eq!(std::fs::read(&key).unwrap(), b"secret");
    }
}
