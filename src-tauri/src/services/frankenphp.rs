//! FrankenPHP from the user's own `frankenphp.exe` (ADR 0019). FrankenPHP's Windows release
//! asset is rebuilt and replaced every day, so it can't be pinned in the catalog; like ngrok
//! (ADR 0011), Devluma never downloads it. The user points Devluma at the folder they
//! unzipped; Devluma checks it (`frankenphp version`, the bundled PHP files) and records a
//! *user-provided* installation that services can use. Devluma makes no integrity claim
//! about those files, and never writes into that folder (php.ini lives in the service folder).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::domain::adapter::AdapterId;
use crate::domain::catalog::{Architecture, version_row_id};
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus, Installation};
use crate::persistence::packages::{self, Package, PackageVersion};
use crate::persistence::{Database, services, settings, sites};

pub const PACKAGE_ID: &str = "frankenphp";
const PATH_SETTING: &str = "frankenphp.path";
const EXE: &str = "frankenphp.exe";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrankenphpStatus {
    /// The frankenphp.exe Devluma uses, if one was chosen.
    pub path: Option<String>,
    pub version: Option<String>,
    /// The PHP version built into it.
    pub php_version: Option<String>,
    /// The installation services are created from.
    pub installation_id: Option<String>,
    /// Why the chosen frankenphp.exe can't be used right now.
    pub problem: Option<String>,
    /// Services that use it (it can't be removed while there are any).
    pub services: Vec<String>,
    /// frankenphp.exe files found in common places.
    pub detected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probed {
    pub version: String,
    pub php_version: String,
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, message, true)
}

/// `frankenphp version` → "FrankenPHP 1.13.5 PHP 8.5.12 Caddy v2.11.9 h1:…".
pub fn parse_version(output: &str) -> Option<Probed> {
    let re = regex::Regex::new(r"(?m)^FrankenPHP v?(\d+\.\d+\.\d+) PHP (\d+\.\d+\.\d+)").ok()?;
    re.captures(output).map(|c| Probed {
        version: c[1].to_owned(),
        php_version: c[2].to_owned(),
    })
}

/// The Windows build's layout: the executable next to the official PHP it embeds.
fn check_folder(dir: &Path) -> Result<(), String> {
    for (file, what) in [
        ("php8ts.dll", "the PHP library"),
        ("php.ini-development", "PHP's php.ini template"),
        ("php.exe", "PHP's command line"),
    ] {
        if !dir.join(file).is_file() {
            return Err(format!(
                "{} has no {file} ({what}). Choose frankenphp.exe inside the folder unzipped from FrankenPHP's Windows zip.",
                dir.display()
            ));
        }
    }
    if !dir.join("ext").is_dir() {
        return Err(format!("{} has no ext folder.", dir.display()));
    }
    Ok(())
}

/// Runs `frankenphp version` (cleared environment, no console window).
fn probe(exe: &Path) -> Result<Probed, String> {
    let out = crate::platform::windows::process::run_captured(
        exe,
        AdapterId::Frankenphp.version_probe().args,
        Duration::from_secs(20),
    )
    .map_err(|e| e.to_string())?;
    if out.exit_code != Some(0) {
        return Err(format!(
            "frankenphp version failed (exit code {:?}): {}",
            out.exit_code,
            out.stderr.trim().chars().take(300).collect::<String>()
        ));
    }
    parse_version(&format!("{}\n{}", out.stdout, out.stderr))
        .ok_or_else(|| "It doesn't report a FrankenPHP and PHP version.".to_owned())
}

/// frankenphp.exe files in common places, for the user to pick from.
pub fn detect() -> Vec<String> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(EXE)).collect())
        .unwrap_or_default();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(u) = env("USERPROFILE") {
        candidates.push(u.join("frankenphp").join(EXE));
        candidates.push(u.join(r"scoop\apps\frankenphp\current").join(EXE));
        candidates.push(u.join(r"Downloads\frankenphp-windows-x86_64").join(EXE));
    }
    if let Some(l) = env("LOCALAPPDATA") {
        candidates.push(l.join(r"Programs\frankenphp").join(EXE));
    }
    for drive in ["C:", "D:"] {
        candidates.push(PathBuf::from(format!(r"{drive}\frankenphp")).join(EXE));
    }
    let mut found: Vec<String> = Vec::new();
    for c in candidates {
        let s = c.display().to_string();
        if c.is_file() && !found.iter().any(|f| f.eq_ignore_ascii_case(&s)) {
            found.push(s);
        }
    }
    found
}

fn installation(c: &rusqlite::Connection) -> crate::persistence::DbResult<Option<Installation>> {
    Ok(installations::list(c)?
        .into_iter()
        .find(|i| i.package_id == PACKAGE_ID))
}

pub fn status(db: &Database) -> Result<FrankenphpStatus, IpcError> {
    let (path, inst, all) = db.read(|c| {
        Ok((
            settings::get::<String>(c, PATH_SETTING)?,
            installation(c)?,
            services::list(c)?,
        ))
    })?;
    let used_by: Vec<String> = inst.as_ref().map_or_else(Vec::new, |i| {
        all.iter()
            .filter(|s| s.package_installation_id == i.id)
            .map(|s| s.instance_name.clone())
            .collect()
    });
    let (probed, problem) = match &path {
        None => (None, None),
        Some(p) => {
            let exe = PathBuf::from(p);
            match exe
                .is_file()
                .then_some(())
                .ok_or_else(|| format!("{p} no longer exists."))
                .and_then(|()| check_folder(exe.parent().unwrap_or(Path::new(""))))
                .and_then(|()| probe(&exe))
            {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e)),
            }
        }
    };
    Ok(FrankenphpStatus {
        path,
        version: probed.as_ref().map(|p| p.version.clone()),
        php_version: probed.map(|p| p.php_version),
        installation_id: inst.map(|i| i.id),
        problem,
        services: used_by,
        detected: detect(),
    })
}

/// Uses `path` as frankenphp.exe after checking it (name, folder layout, `frankenphp
/// version`), and records it as the user-provided FrankenPHP installation. Choosing another
/// copy later updates that installation in place, so existing services keep working.
pub fn set_path(db: &Database, path: &str) -> Result<FrankenphpStatus, IpcError> {
    let exe = PathBuf::from(path.trim());
    let named = exe
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(EXE));
    if !exe.is_absolute() || !named || !exe.is_file() {
        return Err(invalid(
            "Choose the full path of an existing frankenphp.exe.",
        ));
    }
    let dir = exe.parent().unwrap_or(Path::new("")).to_path_buf();
    check_folder(&dir).map_err(invalid)?;
    let probed =
        probe(&exe).map_err(|e| invalid(format!("{} can't be used: {e}", exe.display())))?;

    let version_id = version_row_id(PACKAGE_ID, &probed.version, Architecture::X64);
    let exe_text = exe.display().to_string();
    db.write(|c| {
        packages::upsert(
            c,
            &Package {
                id: PACKAGE_ID.into(),
                display_name: "FrankenPHP".into(),
                category: "web_server".into(),
                description: Some(
                    "Your own frankenphp.exe (Caddy with PHP built in). Devluma doesn't download or verify it."
                        .into(),
                ),
                publisher: Some("FrankenPHP (php/frankenphp), provided by you".into()),
                homepage_url: Some("https://frankenphp.dev/".into()),
                license_id: Some("MIT".into()),
                license_url: None,
                catalog_source: "user-provided".into(),
                catalog_version: "user-provided".into(),
                support_policy_url: None,
            },
        )?;
        packages::upsert_version(
            c,
            &PackageVersion {
                id: version_id.clone(),
                package_id: PACKAGE_ID.into(),
                version: probed.version.clone(),
                architecture: Architecture::X64.as_str().into(),
                os_constraints: serde_json::json!({}),
                source_url: format!("user-provided:{exe_text}"),
                publisher_signature_url: None,
                sha256: None,
                integrity_policy: "user-provided".into(),
                archive_type: "none".into(),
                expected_size_bytes: None,
                release_date: None,
                end_of_life_date: None,
                manifest: serde_json::json!({ "userProvided": true }),
            },
        )?;
        let metadata = serde_json::json!({
            "userProvided": true,
            "executable": exe_text,
            "phpVersion": probed.php_version,
        });
        installations::register_user_provided(
            c,
            installation(c)?.as_ref().map(|i| i.id.as_str()),
            &version_id,
            &dir.display().to_string(),
            &metadata,
        )?;
        settings::set(c, PATH_SETTING, &exe_text)
    })?;
    status(db)
}

/// Forgets the chosen frankenphp.exe. Refused while services use it; the user's folder is
/// never touched.
pub fn remove(db: &Database) -> Result<FrankenphpStatus, IpcError> {
    let current = status(db)?;
    if !current.services.is_empty() {
        return Err(invalid(format!(
            "Remove the services that use FrankenPHP first: {}.",
            current.services.join(", ")
        )));
    }
    db.write(|c| {
        if let Some(inst) = installation(c)? {
            if sites::list(c)?
                .iter()
                .any(|s| s.php_installation_id.as_deref() == Some(inst.id.as_str()))
            {
                return Err(crate::persistence::DbError::Invariant(
                    "A site still uses FrankenPHP.".into(),
                ));
            }
            installations::delete_user_provided(c, &inst.id)?;
        }
        settings::delete(c, PATH_SETTING)
    })?;
    status(db)
}

/// The PHP version a FrankenPHP installation embeds (recorded when it was chosen).
pub fn php_version(inst: &Installation) -> Option<String> {
    inst.metadata
        .get("phpVersion")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// True for the user-provided FrankenPHP installation.
pub fn is_frankenphp(inst: &Installation) -> bool {
    inst.package_id == PACKAGE_ID && inst.status == InstallStatus::Installed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_output_gives_both_versions() {
        let out = "FrankenPHP 1.13.5 PHP 8.5.12 Caddy v2.11.9 h1:bhZtcRKQOq0fuLvEgCdPYvUxXyGn9kzjG4lh2332DTE=\r\n";
        assert_eq!(
            parse_version(out),
            Some(Probed {
                version: "1.13.5".into(),
                php_version: "8.5.12".into()
            })
        );
        assert_eq!(
            parse_version("v2.11.9 h1:x"),
            None,
            "plain Caddy isn't FrankenPHP"
        );
    }

    #[test]
    fn folders_without_the_bundled_php_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(EXE), b"").unwrap();
        let err = check_folder(dir.path()).unwrap_err();
        assert!(err.contains("php8ts.dll"), "{err}");
        for f in ["php8ts.dll", "php.ini-development", "php.exe"] {
            std::fs::write(dir.path().join(f), b"").unwrap();
        }
        assert!(check_folder(dir.path()).unwrap_err().contains("ext folder"));
        std::fs::create_dir(dir.path().join("ext")).unwrap();
        assert!(check_folder(dir.path()).is_ok());
    }

    #[test]
    fn wrong_files_are_refused_before_anything_is_recorded() {
        let db = crate::persistence::test_support::db();
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("caddy.exe");
        std::fs::write(&other, b"").unwrap();
        assert!(set_path(&db, &other.display().to_string()).is_err());
        assert!(set_path(&db, "frankenphp.exe").is_err(), "relative path");
        let s = status(&db).unwrap();
        assert!(s.path.is_none() && s.installation_id.is_none());
    }
}
