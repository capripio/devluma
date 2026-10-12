//! CurrentUser certificate stores (ADR 0010). Adding to or removing from `Root` makes Windows
//! show its own confirmation dialog; other stores (used by tests) don't.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Security::Cryptography::{
    CERT_FIND_SHA1_HASH, CERT_OPEN_STORE_FLAGS, CERT_STORE_ADD_REPLACE_EXISTING,
    CERT_STORE_PROV_SYSTEM_W, CRYPT_INTEGER_BLOB, CertAddEncodedCertificateToStore, CertCloseStore,
    CertDeleteCertificateFromStore, CertFindCertificateInStore, CertFreeCertificateContext,
    CertOpenStore, HCERTSTORE, HCRYPTPROV_LEGACY, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};

/// `CERT_SYSTEM_STORE_CURRENT_USER` (wincrypt.h).
const CURRENT_USER: u32 = 0x0001_0000;

/// The CurrentUser trusted root store.
pub const ROOT: &str = "Root";

struct Store(HCERTSTORE);

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: handle from a successful CertOpenStore, closed once.
        unsafe {
            let _ = CertCloseStore(Some(self.0), 0);
        }
    }
}

fn err(e: windows::core::Error) -> std::io::Error {
    std::io::Error::other(e.message())
}

fn open(name: &str, extra: u32) -> std::io::Result<Option<Store>> {
    let wide: Vec<u16> = OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: the name outlives the call.
    let h = unsafe {
        CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            Some(HCRYPTPROV_LEGACY::default()),
            CERT_OPEN_STORE_FLAGS(CURRENT_USER | extra),
            Some(wide.as_ptr() as *const _),
        )
    }
    .map_err(err)?;
    // With CERT_STORE_DELETE_FLAG the store is deleted and no handle is returned.
    Ok((!h.is_invalid()).then_some(Store(h)))
}

fn store(name: &str) -> std::io::Result<Store> {
    open(name, 0)?.ok_or_else(|| std::io::Error::other("store did not open"))
}

/// SHA-1 thumbprint (Windows' identifier for certificates in stores).
pub fn thumbprint(der: &[u8]) -> Vec<u8> {
    use sha1::Digest;
    sha1::Sha1::digest(der).to_vec()
}

/// Adds (or replaces) a DER certificate. On `Root`, Windows asks the user to confirm.
pub fn add(store_name: &str, der: &[u8]) -> std::io::Result<()> {
    let s = store(store_name)?;
    // SAFETY: `der` outlives the call; no output context requested.
    unsafe {
        CertAddEncodedCertificateToStore(
            Some(s.0),
            X509_ASN_ENCODING,
            der,
            CERT_STORE_ADD_REPLACE_EXISTING,
            None,
        )
    }
    .map_err(err)
}

/// True if a certificate with this SHA-1 thumbprint is in the store.
pub fn contains(store_name: &str, sha1: &[u8]) -> std::io::Result<bool> {
    let s = store(store_name)?;
    let blob = CRYPT_INTEGER_BLOB {
        cbData: sha1.len() as u32,
        pbData: sha1.as_ptr() as *mut u8,
    };
    // SAFETY: blob points to `sha1`, valid for the call; the context is freed.
    unsafe {
        let ctx = CertFindCertificateInStore(
            s.0,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            Some(&blob as *const _ as *const _),
            None,
        );
        if ctx.is_null() {
            return Ok(false);
        }
        let _ = CertFreeCertificateContext(Some(ctx));
        Ok(true)
    }
}

/// Removes the certificate with this thumbprint. Returns false if it wasn't there.
/// On `Root`, Windows asks the user to confirm.
pub fn remove(store_name: &str, sha1: &[u8]) -> std::io::Result<bool> {
    let s = store(store_name)?;
    let blob = CRYPT_INTEGER_BLOB {
        cbData: sha1.len() as u32,
        pbData: sha1.as_ptr() as *mut u8,
    };
    // SAFETY: as above; CertDeleteCertificateFromStore frees the context.
    unsafe {
        let ctx = CertFindCertificateInStore(
            s.0,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_SHA1_HASH,
            Some(&blob as *const _ as *const _),
            None,
        );
        if ctx.is_null() {
            return Ok(false);
        }
        CertDeleteCertificateFromStore(ctx).map_err(err)?;
        Ok(true)
    }
}

/// Deletes a whole (non-system) store. Tests only.
#[cfg(test)]
pub fn delete_store(store_name: &str) {
    let _ = open(
        store_name,
        windows::Win32::Security::Cryptography::CERT_STORE_DELETE_FLAG.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_find_remove_in_a_private_store() {
        let store = format!("DevlumaTest-{}", ulid::Ulid::generate());
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["store-test.test".to_owned()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let der = cert.der().to_vec();
        let thumb = thumbprint(&der);

        assert!(!contains(&store, &thumb).unwrap());
        add(&store, &der).unwrap();
        assert!(contains(&store, &thumb).unwrap());
        assert!(remove(&store, &thumb).unwrap());
        assert!(!contains(&store, &thumb).unwrap());
        assert!(
            !remove(&store, &thumb).unwrap(),
            "removing twice is a no-op"
        );
        delete_store(&store);
    }
}
