//! App and catalog updates (Phase 10). Both are off until the user (or the release
//! configuration) sets an HTTPS URL; nothing is fetched otherwise.
//!
//! - **App updates** use Tauri's updater: the update manifest and installer are signed with
//!   Devluma's update key, whose public half is in `tauri.conf.json`; unsigned or
//!   wrongly signed updates are rejected by the plugin.
//! - **Catalog updates** are separate: `catalog.json` plus `catalog.json.sig` (minisign, made
//!   with `cargo tauri signer sign` and Devluma's *catalog* key). A downloaded catalog is
//!   used only if the signature verifies against the embedded public key, it passes the
//!   same validation as the bundled catalog, and its `catalogVersion` is newer. It is
//!   stored under `<root>/catalog/` and takes effect on the next start.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::supervisor::behavior::{Failure, failure};
use crate::domain::catalog::{self, Catalog, ValidationOptions};
use crate::error::ErrorCode;
use crate::persistence::{Database, DbError, settings};

/// Public half of Devluma's catalog-signing key (minisign key FD09AC61F22FA2A1).
pub const CATALOG_PUBLIC_KEY: &str = "RWShoi/yYawJ/ZpnpNC5g/1z+1BPXYgOtm/bbKSnEzx5DoXS/6uGzdt4";

pub const APP_UPDATE_URL: &str = "updates.app_url";
pub const CATALOG_UPDATE_URL: &str = "updates.catalog_url";
const MAX_CATALOG_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    pub app_url: Option<String>,
    pub catalog_url: Option<String>,
    pub current_version: String,
    pub catalog_version: String,
    /// A verified, newer catalog downloaded and waiting for the next start.
    pub pending_catalog_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCheck {
    pub current_version: String,
    pub available_version: Option<String>,
    pub message: String,
}

fn db_fail(e: DbError) -> Failure {
    let i = crate::error::IpcError::from(e);
    failure(i.code, i.message)
}

fn invalid(msg: impl Into<String>) -> Failure {
    failure(ErrorCode::ValidationError, msg)
}

/// `2026.10.3` > `2026.10.2`; numeric per part, missing parts count as 0.
pub fn version_newer(candidate: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// Verifies a Tauri-signer (`.sig`, base64 of a minisign signature) over `data`.
pub fn verify_signature(public_key: &str, data: &[u8], sig_file: &str) -> Result<(), String> {
    use base64::Engine;
    let pk = minisign_verify::PublicKey::from_base64(public_key).map_err(|e| e.to_string())?;
    let text = base64::engine::general_purpose::STANDARD
        .decode(sig_file.trim())
        .map_err(|_| "the signature file isn't valid base64".to_owned())?;
    let text = String::from_utf8(text).map_err(|_| "the signature file isn't text".to_owned())?;
    let sig = minisign_verify::Signature::decode(&text).map_err(|e| e.to_string())?;
    pk.verify(data, &sig, false).map_err(|e| e.to_string())
}

/// Validates a signed catalog document.
pub fn check_catalog(public_key: &str, json: &[u8], sig: &str) -> Result<Catalog, String> {
    verify_signature(public_key, json, sig).map_err(|e| format!("signature check failed: {e}"))?;
    let text = std::str::from_utf8(json).map_err(|_| "the catalog isn't UTF-8".to_owned())?;
    let c = catalog::parse_and_validate(text, ValidationOptions::default())
        .map_err(|e| e.to_string())?;
    if !c.issues.is_empty() {
        return Err(format!(
            "the catalog has problems: {:?}",
            c.issues[0].message
        ));
    }
    Ok(c)
}

fn stored_paths(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join("catalog");
    (dir.join("catalog.json"), dir.join("catalog.json.sig"))
}

/// The stored downloaded catalog, if it still verifies and is newer than `bundled`.
pub fn stored_catalog(root: &Path, public_key: &str, bundled: &Catalog) -> Option<Catalog> {
    let (json_path, sig_path) = stored_paths(root);
    let json = std::fs::read(json_path).ok()?;
    let sig = std::fs::read_to_string(sig_path).ok()?;
    let c = check_catalog(public_key, &json, &sig).ok()?;
    version_newer(&c.catalog_version, &bundled.catalog_version).then_some(c)
}

pub struct UpdateService {
    db: Arc<Database>,
    root: PathBuf,
    current_version: String,
    catalog_version: String,
    catalog_public_key: String,
}

fn check_url(url: &str) -> Result<Option<String>, Failure> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    let parsed = url::Url::parse(url).map_err(|_| invalid("That isn't a valid URL."))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("Use an https:// URL without credentials."));
    }
    Ok(Some(url.to_owned()))
}

impl UpdateService {
    pub fn new(
        db: Arc<Database>,
        root: PathBuf,
        current_version: String,
        catalog_version: String,
    ) -> Arc<Self> {
        Arc::new(Self::with_key(
            db,
            root,
            current_version,
            catalog_version,
            CATALOG_PUBLIC_KEY,
        ))
    }

    pub fn with_key(
        db: Arc<Database>,
        root: PathBuf,
        current_version: String,
        catalog_version: String,
        catalog_public_key: &str,
    ) -> Self {
        Self {
            db,
            root,
            current_version,
            catalog_version,
            catalog_public_key: catalog_public_key.to_owned(),
        }
    }

    pub fn settings(&self) -> Result<UpdateSettings, Failure> {
        let (app_url, catalog_url) = self
            .db
            .read(|c| {
                Ok((
                    settings::get::<String>(c, APP_UPDATE_URL)?,
                    settings::get::<String>(c, CATALOG_UPDATE_URL)?,
                ))
            })
            .map_err(db_fail)?;
        let (json_path, sig_path) = stored_paths(&self.root);
        let pending = std::fs::read(&json_path)
            .ok()
            .zip(std::fs::read_to_string(&sig_path).ok())
            .and_then(|(j, s)| check_catalog(&self.catalog_public_key, &j, &s).ok())
            .map(|c| c.catalog_version)
            .filter(|v| version_newer(v, &self.catalog_version));
        Ok(UpdateSettings {
            app_url: app_url.filter(|u| !u.is_empty()),
            catalog_url: catalog_url.filter(|u| !u.is_empty()),
            current_version: self.current_version.clone(),
            catalog_version: self.catalog_version.clone(),
            pending_catalog_version: pending,
        })
    }

    pub fn set_urls(&self, app_url: &str, catalog_url: &str) -> Result<UpdateSettings, Failure> {
        let app = check_url(app_url)?.unwrap_or_default();
        let cat = check_url(catalog_url)?.unwrap_or_default();
        self.db
            .write(|c| {
                settings::set(c, APP_UPDATE_URL, &app)?;
                settings::set(c, CATALOG_UPDATE_URL, &cat)
            })
            .map_err(db_fail)?;
        self.settings()
    }

    /// Downloads, verifies and stores a newer catalog (used from the next start).
    pub async fn check_catalog_update(&self) -> Result<CatalogCheck, Failure> {
        let url = self
            .settings()?
            .catalog_url
            .ok_or_else(|| invalid("Catalog updates aren't configured for this build."))?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let fetch = |u: String| {
            let client = client.clone();
            async move {
                let resp = client
                    .get(&u)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status());
                let resp = resp.map_err(|e| {
                    failure(
                        ErrorCode::NetworkError,
                        format!("Could not download {u}: {e}"),
                    )
                })?;
                if resp
                    .content_length()
                    .is_some_and(|n| n as usize > MAX_CATALOG_BYTES)
                {
                    return Err(invalid("The catalog is unexpectedly large."));
                }
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| failure(ErrorCode::NetworkError, e.to_string()))?;
                if bytes.len() > MAX_CATALOG_BYTES {
                    return Err(invalid("The catalog is unexpectedly large."));
                }
                Ok(bytes.to_vec())
            }
        };
        let json = fetch(url.clone()).await?;
        let sig = fetch(format!("{url}.sig")).await?;
        let sig = String::from_utf8(sig).map_err(|_| invalid("The signature file isn't text."))?;
        self.accept_catalog(&json, &sig)
    }

    /// Verification + storage, separate from the download for testing.
    pub fn accept_catalog(&self, json: &[u8], sig: &str) -> Result<CatalogCheck, Failure> {
        let c = check_catalog(&self.catalog_public_key, json, sig).map_err(|e| {
            failure(
                ErrorCode::IntegrityError,
                format!("The downloaded catalog was rejected: {e}."),
            )
        })?;
        if !version_newer(&c.catalog_version, &self.catalog_version) {
            return Ok(CatalogCheck {
                current_version: self.catalog_version.clone(),
                available_version: None,
                message: format!(
                    "The package catalog is up to date ({}).",
                    self.catalog_version
                ),
            });
        }
        let (json_path, sig_path) = stored_paths(&self.root);
        let io = |e: std::io::Error| {
            failure(
                ErrorCode::Internal,
                format!("Could not save the catalog: {e}"),
            )
        };
        std::fs::create_dir_all(json_path.parent().unwrap_or(&self.root)).map_err(io)?;
        // Signature last: a half-written pair never verifies, so it's never used.
        std::fs::write(&json_path, json).map_err(io)?;
        std::fs::write(&sig_path, sig).map_err(io)?;
        Ok(CatalogCheck {
            current_version: self.catalog_version.clone(),
            available_version: Some(c.catalog_version.clone()),
            message: format!(
                "Catalog {} was verified and saved. Restart Devluma to use it.",
                c.catalog_version
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    /// Throwaway key that signed `tests/fixtures/catalog-2099.json` (private half discarded).
    const TEST_KEY: &str = "RWSSrN4Bx4duhIOx5V/rT8szcx5pCP+DJaetOmFQ0hseMOWzyKx6hA0s";
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/catalog-2099.json");
    const FIXTURE_SIG: &str = include_str!("../../tests/fixtures/catalog-2099.json.sig");

    fn svc(dir: &Path) -> UpdateService {
        UpdateService::with_key(
            Arc::new(db()),
            dir.to_path_buf(),
            "0.1.0".into(),
            "2026.10.3".into(),
            TEST_KEY,
        )
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_newer("2026.10.10", "2026.10.3"));
        assert!(!version_newer("2026.10.3", "2026.10.3"));
        assert!(!version_newer("2026.9.9", "2026.10.1"));
        assert!(version_newer("2027", "2026.12.31"));
    }

    #[test]
    fn signed_catalog_is_accepted_stored_and_used_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let s = svc(dir.path());
        let r = s.accept_catalog(FIXTURE, FIXTURE_SIG).unwrap();
        assert_eq!(r.available_version.as_deref(), Some("2099.1.1"));
        assert_eq!(
            s.settings().unwrap().pending_catalog_version.as_deref(),
            Some("2099.1.1")
        );
        let bundled = crate::services::catalog::load();
        let used = stored_catalog(dir.path(), TEST_KEY, &bundled).expect("newer verified catalog");
        assert_eq!(used.catalog_version, "2099.1.1");
    }

    #[test]
    fn tampered_or_wrongly_signed_catalogs_are_rejected_and_never_stored() {
        let dir = tempfile::tempdir().unwrap();
        let s = svc(dir.path());
        let mut tampered = FIXTURE.to_vec();
        let pos = tampered.windows(4).position(|w| w == b"2099").unwrap();
        tampered[pos + 3] = b'8';
        let err = s.accept_catalog(&tampered, FIXTURE_SIG).unwrap_err();
        assert_eq!(err.code, ErrorCode::IntegrityError);
        let other_key = UpdateService::with_key(
            Arc::new(db()),
            dir.path().to_path_buf(),
            "0.1.0".into(),
            "2026.10.3".into(),
            CATALOG_PUBLIC_KEY,
        );
        assert!(
            other_key.accept_catalog(FIXTURE, FIXTURE_SIG).is_err(),
            "signed by a different key"
        );
        assert!(s.accept_catalog(FIXTURE, "not base64 !!").is_err());
        assert!(!dir.path().join("catalog").join("catalog.json").exists());

        // A stored file that was modified later is ignored at startup.
        s.accept_catalog(FIXTURE, FIXTURE_SIG).unwrap();
        std::fs::write(dir.path().join("catalog").join("catalog.json"), &tampered).unwrap();
        let bundled = crate::services::catalog::load();
        assert!(stored_catalog(dir.path(), TEST_KEY, &bundled).is_none());
    }

    #[test]
    fn older_catalogs_are_not_installed_and_urls_must_be_https() {
        let dir = tempfile::tempdir().unwrap();
        let s = UpdateService::with_key(
            Arc::new(db()),
            dir.path().to_path_buf(),
            "0.1.0".into(),
            "2100.1.1".into(),
            TEST_KEY,
        );
        assert_eq!(
            s.accept_catalog(FIXTURE, FIXTURE_SIG)
                .unwrap()
                .available_version,
            None
        );
        assert!(s.set_urls("http://example.com/x", "").is_err());
        assert!(s.set_urls("https://user:pw@example.com/x", "").is_err());
        let set = s
            .set_urls("https://example.com/{{target}}/{{current_version}}", "")
            .unwrap();
        assert!(set.app_url.is_some() && set.catalog_url.is_none());
    }
}
