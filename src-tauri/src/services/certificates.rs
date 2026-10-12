//! Local development certificates (ADR 0010, app_flow.md §11).
//!
//! - One local CA, name-constrained to local-only suffixes; its private key is sealed with
//!   DPAPI and only Devluma uses it, to sign.
//! - Site certificates are signed by that CA; their keys are PEM files restricted to the
//!   current user (Nginx must read them).
//! - Generating, trusting, removing trust and deleting are four separate actions. Removing
//!   trust never deletes a key; deleting is refused while a certificate is still in use.

use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::{Duration as TimeDuration, OffsetDateTime};

use crate::domain::domain_name;
use crate::error::ErrorCode;
use crate::persistence::certificates::{self as repo, Certificate, KIND_CA, KIND_SIGNED};
use crate::persistence::{Database, DbError, new_id, sites};
use crate::platform::windows::{acl, certstore, dpapi};
use crate::services::supervisor::behavior::{Failure, failure};
use crate::state::DataPaths;

/// Suffixes the CA may issue for (X.509 permitted DNS subtrees).
pub const LOCAL_SUFFIXES: [&str; 6] = [
    "test",
    "localhost",
    "internal",
    "example",
    "invalid",
    "local",
];
const CA_YEARS: i64 = 10;
const SITE_DAYS: i64 = 397;
pub const EXPIRY_WARNING_DAYS: i64 = 30;

fn db_fail(e: DbError) -> Failure {
    let i = crate::error::IpcError::from(e);
    failure(i.code, i.message)
}

fn io(context: &str) -> impl FnOnce(std::io::Error) -> Failure + '_ {
    move |e| failure(ErrorCode::Internal, format!("Could not {context}: {e}"))
}

fn rcgen_fail(context: &str) -> impl FnOnce(rcgen::Error) -> Failure + '_ {
    move |e| failure(ErrorCode::Internal, format!("Could not {context}: {e}"))
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateView {
    #[serde(flatten)]
    pub certificate: Certificate,
    pub days_left: i64,
    pub expiring_soon: bool,
    pub expired: bool,
    /// Live state of the CA in the trust store (CA only).
    pub trusted_now: bool,
    pub bound_sites: Vec<String>,
    pub key_protection: String,
}

pub struct CertificateService {
    db: std::sync::Arc<Database>,
    paths: DataPaths,
    /// `Root` in production; a private store in tests.
    trust_store: String,
}

impl CertificateService {
    pub fn new(db: std::sync::Arc<Database>, paths: DataPaths, trust_store: &str) -> Self {
        Self {
            db,
            paths,
            trust_store: trust_store.to_owned(),
        }
    }

    fn dir(&self) -> PathBuf {
        self.paths.root.join("certificates")
    }

    pub fn ca(&self) -> Result<Option<Certificate>, Failure> {
        self.db.read(repo::ca).map_err(db_fail)
    }

    /// Creates the local CA if there isn't one yet.
    pub fn ensure_ca(&self) -> Result<Certificate, Failure> {
        if let Some(ca) = self.ca()? {
            return Ok(ca);
        }
        let key = KeyPair::generate().map_err(rcgen_fail("generate the CA key"))?;
        let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".into());
        let mut params =
            CertificateParams::new(Vec::<String>::new()).map_err(rcgen_fail("prepare the CA"))?;
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            format!("Devluma Local Development CA ({computer})"),
        );
        dn.push(DnType::OrganizationName, "Devluma (local development only)");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params.name_constraints = Some(NameConstraints {
            permitted_subtrees: LOCAL_SUFFIXES
                .iter()
                .map(|s| GeneralSubtree::DnsName((*s).into()))
                .collect(),
            excluded_subtrees: Vec::new(),
        });
        let now = OffsetDateTime::now_utc();
        params.not_before = now - TimeDuration::days(1);
        params.not_after = now + TimeDuration::days(365 * CA_YEARS);
        let cert = params
            .self_signed(&key)
            .map_err(rcgen_fail("create the CA certificate"))?;

        let dir = self.dir().join("ca");
        std::fs::create_dir_all(&dir).map_err(io("create the certificates folder"))?;
        let (cert_path, key_path) = (dir.join("ca.pem"), dir.join("ca.key.dpapi"));
        std::fs::write(&cert_path, cert.pem()).map_err(io("save the CA certificate"))?;
        std::fs::write(dir.join("ca.der"), cert.der().as_ref())
            .map_err(io("save the CA certificate"))?;
        let sealed =
            dpapi::protect(key.serialize_pem().as_bytes()).map_err(io("protect the CA key"))?;
        std::fs::write(&key_path, sealed).map_err(io("save the CA key"))?;
        acl::restrict_to_current_user(&key_path).map_err(io("restrict access to the CA key"))?;

        let record = Certificate {
            id: new_id(),
            common_name: format!("Devluma Local Development CA ({computer})"),
            sans: Vec::new(),
            certificate_path: cert_path.display().to_string(),
            private_key_path: key_path.display().to_string(),
            issuer_type: KIND_CA.into(),
            fingerprint_sha256: hex::encode(Sha256::digest(cert.der())),
            not_before: rfc3339(params.not_before),
            not_after: rfc3339(params.not_after),
            trust_store: None,
            trusted: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        self.db.write(|c| repo::upsert(c, &record)).map_err(db_fail)
    }

    fn ca_issuer(&self, ca: &Certificate) -> Result<Issuer<'static, KeyPair>, Failure> {
        let sealed = std::fs::read(&ca.private_key_path).map_err(io("read the CA key"))?;
        let pem = dpapi::unprotect(&sealed).map_err(|_| {
            failure(
                ErrorCode::PermissionDenied,
                "The CA key can't be unlocked by this Windows account. Create a new CA.",
            )
        })?;
        let key = KeyPair::from_pem(&String::from_utf8_lossy(&pem))
            .map_err(rcgen_fail("load the CA key"))?;
        let cert_pem =
            std::fs::read_to_string(&ca.certificate_path).map_err(io("read the CA certificate"))?;
        Issuer::from_ca_cert_pem(&cert_pem, key).map_err(rcgen_fail("load the CA certificate"))
    }

    /// Normalized, de-duplicated domains the CA is allowed to sign.
    pub fn check_domains(domains: &[String]) -> Result<Vec<String>, Failure> {
        let mut out: Vec<String> = Vec::new();
        for d in domains {
            let n = domain_name::check(d)
                .map_err(|m| failure(ErrorCode::ValidationError, m))?
                .normalized;
            let tld = n.rsplit('.').next().unwrap_or_default();
            if !LOCAL_SUFFIXES.contains(&tld) {
                return Err(failure(
                    ErrorCode::ValidationError,
                    format!(
                        "Devluma's local CA only issues certificates for local domains ({}). {n} isn't one.",
                        LOCAL_SUFFIXES.map(|s| format!(".{s}")).join(", ")
                    ),
                ));
            }
            if !out.contains(&n) {
                out.push(n);
            }
        }
        if out.is_empty() {
            return Err(failure(
                ErrorCode::ValidationError,
                "Enter at least one domain.",
            ));
        }
        Ok(out)
    }

    fn write_site_cert(
        &self,
        id: &str,
        domains: &[String],
    ) -> Result<(PathBuf, PathBuf, OffsetDateTime, OffsetDateTime, String), Failure> {
        let ca = self.ensure_ca()?;
        let issuer = self.ca_issuer(&ca)?;
        let key = KeyPair::generate().map_err(rcgen_fail("generate a key"))?;
        let mut params = CertificateParams::new(domains.to_vec())
            .map_err(rcgen_fail("prepare the certificate"))?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, domains[0].clone());
        params.distinguished_name = dn;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - TimeDuration::days(1);
        params.not_after = now + TimeDuration::days(SITE_DAYS);
        let cert = params
            .signed_by(&key, &issuer)
            .map_err(rcgen_fail("sign the certificate"))?;

        let dir = self.dir().join(id);
        std::fs::create_dir_all(&dir).map_err(io("create the certificate folder"))?;
        let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
        // Key first, restricted before any content is written; then replace atomically.
        let tmp_key = dir.join("key.pem.devluma-tmp");
        std::fs::write(&tmp_key, b"").map_err(io("create the key file"))?;
        acl::restrict_to_current_user(&tmp_key).map_err(io("restrict access to the key"))?;
        std::fs::write(&tmp_key, key.serialize_pem()).map_err(io("save the key"))?;
        std::fs::rename(&tmp_key, &key_path).map_err(io("save the key"))?;
        acl::restrict_to_current_user(&key_path).map_err(io("restrict access to the key"))?;
        // Leaf + CA, so clients that don't have the CA yet still see the full chain.
        let ca_pem =
            std::fs::read_to_string(&ca.certificate_path).map_err(io("read the CA certificate"))?;
        std::fs::write(&cert_path, format!("{}{}", cert.pem(), ca_pem))
            .map_err(io("save the certificate"))?;
        Ok((
            cert_path,
            key_path,
            params.not_before,
            params.not_after,
            hex::encode(Sha256::digest(cert.der())),
        ))
    }

    /// Generates a CA-signed certificate for `domains`. Doesn't trust anything.
    pub fn issue(&self, domains: &[String]) -> Result<Certificate, Failure> {
        let domains = Self::check_domains(domains)?;
        let id = new_id();
        let (cert_path, key_path, nb, na, fp) = self.write_site_cert(&id, &domains)?;
        let record = Certificate {
            id,
            common_name: domains[0].clone(),
            sans: domains,
            certificate_path: cert_path.display().to_string(),
            private_key_path: key_path.display().to_string(),
            issuer_type: KIND_SIGNED.into(),
            fingerprint_sha256: fp,
            not_before: rfc3339(nb),
            not_after: rfc3339(na),
            trust_store: None,
            trusted: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        self.db.write(|c| repo::upsert(c, &record)).map_err(db_fail)
    }

    /// A current certificate covering `domain`, or a new one.
    pub fn for_domain(&self, domain: &str) -> Result<Certificate, Failure> {
        match self.find_for_domain(domain)? {
            Some(c) => Ok(c),
            None => self.issue(&[domain.to_owned()]),
        }
    }

    /// A current certificate covering `domain`, without creating anything (for previews).
    pub fn find_for_domain(&self, domain: &str) -> Result<Option<Certificate>, Failure> {
        let now = OffsetDateTime::now_utc();
        Ok(self
            .db
            .read(repo::list)
            .map_err(db_fail)?
            .into_iter()
            .find(|c| {
                c.issuer_type == KIND_SIGNED
                    && c.sans.iter().any(|s| s == domain)
                    && days_left(&c.not_after, now) > EXPIRY_WARNING_DAYS
                    && Path::new(&c.certificate_path).is_file()
                    && Path::new(&c.private_key_path).is_file()
            }))
    }

    /// Re-issues a site certificate in place (same id and file paths).
    pub fn renew(&self, id: &str) -> Result<Certificate, Failure> {
        let mut c = self.db.read(|conn| repo::get(conn, id)).map_err(db_fail)?;
        if c.issuer_type != KIND_SIGNED {
            return Err(failure(
                ErrorCode::ValidationError,
                "Only site certificates can be renewed.",
            ));
        }
        let (_, _, nb, na, fp) = self.write_site_cert(&c.id, &c.sans)?;
        c.not_before = rfc3339(nb);
        c.not_after = rfc3339(na);
        c.fingerprint_sha256 = fp;
        self.db
            .write(|conn| repo::upsert(conn, &c))
            .map_err(db_fail)
    }

    fn ca_thumbprint(ca: &Certificate) -> Result<Vec<u8>, Failure> {
        let der_path = Path::new(&ca.certificate_path).with_extension("der");
        let der = std::fs::read(der_path).map_err(io("read the CA certificate"))?;
        Ok(certstore::thumbprint(&der))
    }

    /// Whether the CA is in the trust store right now (it can change outside Devluma).
    pub fn ca_trusted_now(&self, ca: &Certificate) -> bool {
        Self::ca_thumbprint(ca)
            .ok()
            .and_then(|t| certstore::contains(&self.trust_store, &t).ok())
            .unwrap_or(false)
    }

    /// Adds the CA to the current user's trusted roots. Windows asks the user to confirm.
    pub fn trust_ca(&self) -> Result<Certificate, Failure> {
        let ca = self
            .ca()?
            .ok_or_else(|| failure(ErrorCode::ValidationError, "There's no local CA yet."))?;
        let der = std::fs::read(Path::new(&ca.certificate_path).with_extension("der"))
            .map_err(io("read the CA certificate"))?;
        certstore::add(&self.trust_store, &der).map_err(|_| {
            failure(
                ErrorCode::PermissionDenied,
                "The CA wasn't added to your trusted certificates (Windows' confirmation was declined or failed).",
            )
        })?;
        if !self.ca_trusted_now(&ca) {
            return Err(failure(
                ErrorCode::PermissionDenied,
                "Windows didn't add the CA to your trusted certificates.",
            ));
        }
        let store = format!("CurrentUser\\{}", self.trust_store);
        self.db
            .write(|c| repo::set_trust(c, &ca.id, Some(&store), true))
            .map_err(db_fail)?;
        self.db.read(|c| repo::get(c, &ca.id)).map_err(db_fail)
    }

    /// Removes the CA from the trusted roots. Keys and files are kept.
    pub fn untrust_ca(&self) -> Result<Certificate, Failure> {
        let ca = self
            .ca()?
            .ok_or_else(|| failure(ErrorCode::ValidationError, "There's no local CA."))?;
        let thumb = Self::ca_thumbprint(&ca)?;
        if certstore::contains(&self.trust_store, &thumb).unwrap_or(false) {
            certstore::remove(&self.trust_store, &thumb).map_err(|_| {
                failure(
                    ErrorCode::PermissionDenied,
                    "The CA wasn't removed (Windows' confirmation was declined or failed).",
                )
            })?;
        }
        if self.ca_trusted_now(&ca) {
            return Err(failure(
                ErrorCode::PermissionDenied,
                "The CA is still trusted; Windows didn't remove it.",
            ));
        }
        self.db
            .write(|c| repo::set_trust(c, &ca.id, None, false))
            .map_err(db_fail)?;
        self.db.read(|c| repo::get(c, &ca.id)).map_err(db_fail)
    }

    /// Deletes a certificate and its key — only when nothing depends on it.
    pub fn delete(&self, id: &str) -> Result<(), Failure> {
        let c = self.db.read(|conn| repo::get(conn, id)).map_err(db_fail)?;
        if c.issuer_type == KIND_CA {
            if self.ca_trusted_now(&c) {
                return Err(failure(
                    ErrorCode::ValidationError,
                    "Remove trust for the CA before deleting it.",
                ));
            }
            if self
                .db
                .read(repo::list)
                .map_err(db_fail)?
                .iter()
                .any(|x| x.issuer_type == KIND_SIGNED)
            {
                return Err(failure(
                    ErrorCode::ValidationError,
                    "Delete the site certificates issued by this CA first.",
                ));
            }
        } else {
            let bound = self
                .db
                .read(|conn| repo::bound_sites(conn, id))
                .map_err(db_fail)?;
            if !bound.is_empty() {
                let domains: Vec<String> = bound
                    .iter()
                    .filter_map(|s| {
                        self.db
                            .read(|conn| sites::get(conn, s))
                            .ok()
                            .map(|s| s.domain)
                    })
                    .collect();
                return Err(failure(
                    ErrorCode::ValidationError,
                    format!(
                        "This certificate is used by {}. Turn off HTTPS for those sites first.",
                        domains.join(", ")
                    ),
                ));
            }
        }
        if let Some(dir) = Path::new(&c.certificate_path).parent()
            && crate::security::paths::ensure_within(&self.dir(), dir).is_ok()
        {
            std::fs::remove_dir_all(dir).map_err(io("delete the certificate files"))?;
        }
        self.db
            .write(|conn| repo::delete(conn, id))
            .map_err(db_fail)
    }

    pub fn list(&self) -> Result<Vec<CertificateView>, Failure> {
        let now = OffsetDateTime::now_utc();
        let all = self.db.read(repo::list).map_err(db_fail)?;
        all.into_iter()
            .map(|c| {
                let days = days_left(&c.not_after, now);
                let bound = self
                    .db
                    .read(|conn| repo::bound_sites(conn, &c.id))
                    .map_err(db_fail)?;
                let bound_sites = bound
                    .iter()
                    .filter_map(|s| {
                        self.db
                            .read(|conn| sites::get(conn, s))
                            .ok()
                            .map(|s| s.domain)
                    })
                    .collect();
                let is_ca = c.issuer_type == KIND_CA;
                Ok(CertificateView {
                    trusted_now: is_ca && self.ca_trusted_now(&c),
                    days_left: days,
                    expiring_soon: days <= EXPIRY_WARNING_DAYS,
                    expired: days < 0,
                    key_protection: if is_ca {
                        "Encrypted with Windows DPAPI for your account".into()
                    } else {
                        "Readable only by your Windows account".into()
                    },
                    bound_sites,
                    certificate: c,
                })
            })
            .collect()
    }
}

fn days_left(not_after: &str, now: OffsetDateTime) -> i64 {
    OffsetDateTime::parse(not_after, &time::format_description::well_known::Rfc3339)
        .map(|t| (t - now).whole_days())
        .unwrap_or(-1)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A service using a private trust store (no Windows dialogs). Drop the guard to delete it.
    pub struct TestStore(pub String);
    impl Drop for TestStore {
        fn drop(&mut self) {
            certstore::delete_store(&self.0);
        }
    }

    pub fn service(
        db: std::sync::Arc<Database>,
        paths: &DataPaths,
    ) -> (CertificateService, TestStore) {
        let store = format!("DevlumaTest-{}", ulid::Ulid::generate());
        (
            CertificateService::new(db, paths.clone(), &store),
            TestStore(store),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::test_support::service;
    use super::*;
    use crate::persistence::test_support::db;

    fn setup() -> (
        tempfile::TempDir,
        CertificateService,
        test_support::TestStore,
        Arc<Database>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let database = Arc::new(db());
        let (svc, store) = service(Arc::clone(&database), &paths);
        (dir, svc, store, database)
    }

    #[test]
    fn ca_key_is_sealed_and_site_keys_are_restricted() {
        let (_d, svc, _s, _db) = setup();
        let site = svc
            .issue(&["Shop.Test".into(), "api.shop.test".into()])
            .unwrap();
        assert_eq!(site.sans, vec!["shop.test", "api.shop.test"]);
        let ca = svc.ca().unwrap().unwrap();

        let ca_key = std::fs::read(&ca.private_key_path).unwrap();
        assert!(
            !String::from_utf8_lossy(&ca_key).contains("PRIVATE KEY"),
            "CA key is not stored in plain text"
        );
        assert_eq!(
            acl::dacl_entry_count(Path::new(&site.private_key_path)).unwrap(),
            1
        );
        assert!(
            std::fs::read_to_string(&site.private_key_path)
                .unwrap()
                .contains("PRIVATE KEY")
        );
        // cert.pem carries leaf + CA.
        assert_eq!(
            std::fs::read_to_string(&site.certificate_path)
                .unwrap()
                .matches("BEGIN CERTIFICATE")
                .count(),
            2
        );
    }

    #[test]
    fn only_local_domains_are_issued() {
        let (_d, svc, _s, _db) = setup();
        let err = svc.issue(&["bank.com".into()]).unwrap_err();
        assert!(
            err.message
                .contains("only issues certificates for local domains")
        );
        assert!(svc.issue(&["not a domain".into()]).is_err());
        assert!(
            svc.ca().unwrap().is_none(),
            "nothing was created for a refused request"
        );
    }

    #[test]
    fn issued_certificates_verify_against_the_ca_with_name_constraints() {
        use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
        let (_d, svc, _s, _db) = setup();
        let site = svc.issue(&["shop.test".into()]).unwrap();
        let ca = svc.ca().unwrap().unwrap();
        let ca_der = std::fs::read(Path::new(&ca.certificate_path).with_extension("der")).unwrap();
        let pem = std::fs::read(&site.certificate_path).unwrap();
        let leaf = rustls_pemfile_first(&pem);

        let anchor = webpki::anchor_from_trusted_cert(&CertificateDer::from(ca_der.as_slice()))
            .unwrap()
            .to_owned();
        let leaf_der = CertificateDer::from(leaf);
        let ee = webpki::EndEntityCert::try_from(&leaf_der).unwrap();
        let now = UnixTime::now();
        ee.verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &[anchor],
            &[],
            now,
            webpki::KeyUsage::server_auth(),
            None,
            None,
        )
        .expect("chain validates, including the CA's name constraints");
        ee.verify_is_valid_for_subject_name(&ServerName::try_from("shop.test").unwrap())
            .unwrap();
        assert!(
            ee.verify_is_valid_for_subject_name(&ServerName::try_from("other.test").unwrap())
                .is_err()
        );
    }

    /// First certificate in a PEM bundle, as DER.
    fn rustls_pemfile_first(pem: &[u8]) -> Vec<u8> {
        use rustls_pki_types::pem::PemObject;
        rustls_pki_types::CertificateDer::pem_slice_iter(pem)
            .next()
            .unwrap()
            .unwrap()
            .to_vec()
    }

    #[test]
    fn generation_trust_untrust_and_deletion_are_separate() {
        let (_d, svc, _s, _db) = setup();
        let site = svc.issue(&["shop.test".into()]).unwrap();
        let ca = svc.ca().unwrap().unwrap();
        assert!(!svc.ca_trusted_now(&ca), "generating never trusts");

        assert!(svc.trust_ca().unwrap().trusted);
        assert!(svc.ca_trusted_now(&ca));
        assert!(
            svc.delete(&ca.id)
                .unwrap_err()
                .message
                .contains("Remove trust")
        );

        assert!(!svc.untrust_ca().unwrap().trusted);
        assert!(
            Path::new(&ca.private_key_path).exists(),
            "removing trust keeps the key"
        );

        assert!(
            svc.delete(&ca.id)
                .unwrap_err()
                .message
                .contains("site certificates")
        );
        svc.delete(&site.id).unwrap();
        assert!(!Path::new(&site.private_key_path).exists());
        svc.delete(&ca.id).unwrap();
        assert!(svc.ca().unwrap().is_none());
    }

    #[test]
    fn renewal_keeps_identity_and_paths() {
        let (_d, svc, _s, _db) = setup();
        let c = svc.issue(&["shop.test".into()]).unwrap();
        let before = std::fs::read(&c.private_key_path).unwrap();
        let r = svc.renew(&c.id).unwrap();
        assert_eq!(
            (r.id.as_str(), r.certificate_path.as_str()),
            (c.id.as_str(), c.certificate_path.as_str())
        );
        assert_ne!(
            std::fs::read(&r.private_key_path).unwrap(),
            before,
            "new key"
        );
        assert_eq!(
            acl::dacl_entry_count(Path::new(&r.private_key_path)).unwrap(),
            1
        );
        assert_eq!(
            svc.for_domain("shop.test").unwrap().id,
            c.id,
            "reused while valid"
        );
    }
}
