//! Windows Credential Manager (generic credentials) for secrets such as the ngrok authtoken.
//! Stored per user and per machine (`CRED_PERSIST_LOCAL_MACHINE`: not roamed). Devluma's
//! database only keeps the credential's target name (`secret_references`).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
    CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Creates or replaces the generic credential `target`.
pub fn write(target: &str, user: &str, secret: &[u8]) -> std::io::Result<()> {
    let mut target_w = wide(target);
    let mut user_w = wide(user);
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_w.as_mut_ptr()),
        CredentialBlobSize: u32::try_from(secret.len()).map_err(std::io::Error::other)?,
        CredentialBlob: secret.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user_w.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: every pointer in `cred` refers to a buffer that outlives the call; the API
    // copies the data.
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| std::io::Error::from_raw_os_error(e.code().0))
}

/// The secret stored under `target`, if any.
pub fn read(target: &str) -> Option<Vec<u8>> {
    let target_w = wide(target);
    let mut out: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target_w` is NUL-terminated; on success `out` is freed with CredFree below.
    unsafe { CredReadW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, None, &mut out) }.ok()?;
    // SAFETY: `out` points to a CREDENTIALW allocated by the API until CredFree.
    let secret = unsafe {
        let c = &*out;
        let blob =
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
        CredFree(out.cast());
        blob
    };
    Some(secret)
}

/// Deletes the credential. Returns false if it didn't exist.
pub fn delete(target: &str) -> bool {
    let target_w = wide(target);
    // SAFETY: NUL-terminated name outlives the call.
    unsafe { CredDeleteW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, None) }.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_round_trip_and_delete() {
        let target = format!("Devluma/test/{}", ulid::Ulid::generate());
        assert_eq!(read(&target), None);
        write(&target, "devluma", b"first").unwrap();
        write(&target, "devluma", b"second-value").unwrap();
        assert_eq!(read(&target).as_deref(), Some(&b"second-value"[..]));
        assert!(delete(&target));
        assert!(!delete(&target));
        assert_eq!(read(&target), None);
    }
}
