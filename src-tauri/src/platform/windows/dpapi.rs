//! Windows DPAPI (current-user scope): data encrypted here can only be decrypted by the same
//! Windows account on this computer. Used for the local CA's private key (ADR 0010).

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Copies and frees a blob returned by DPAPI.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY (caller): `out` was filled by a successful DPAPI call.
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    }
    v
}

pub fn protect(plain: &[u8]) -> std::io::Result<Vec<u8>> {
    let input = blob(plain);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: input points to `plain`, which outlives the call; no UI is allowed.
    unsafe {
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| std::io::Error::other(e.message()))?;
        Ok(take(out))
    }
}

pub fn unprotect(sealed: &[u8]) -> std::io::Result<Vec<u8>> {
    let input = blob(sealed);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as above.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| std::io::Error::other(e.message()))?;
        Ok(take(out))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips_and_detects_tampering() {
        let sealed = super::protect(b"-----BEGIN PRIVATE KEY-----").unwrap();
        assert!(
            !sealed.windows(5).any(|w| w == b"BEGIN"),
            "ciphertext doesn't contain the plaintext"
        );
        assert_eq!(
            super::unprotect(&sealed).unwrap(),
            b"-----BEGIN PRIVATE KEY-----"
        );
        let mut bad = sealed.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(super::unprotect(&bad).is_err());
    }
}
