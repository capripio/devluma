//! Uninstalling a package version (app_flow.md §9, prd.md §8: data is retained by default).
//!
//! Only the app-managed install folder under `packages/` is deleted. Data folders, project
//! files, backups, certificates and logs are never touched. Uninstall is refused while a
//! service or site still uses the version; project pins and the global default that point at
//! it are cleared (and listed in the preview). Stopped PHP FastCGI backends that Devluma
//! created on demand for this version are removed with it.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::journal::{Journal, ProgressSink};
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::{NewOperation, Operation};
use crate::persistence::{Database, projects, services, sites};
use crate::security::paths as pathpolicy;

pub const OPERATION_TYPE: &str = "uninstall_package";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPlan {
    pub installation_id: String,
    pub package_id: String,
    pub version: String,
    /// The folder that will be deleted.
    pub install_path: String,
    /// Services that must be removed first (their data folders are kept when they are).
    pub blocking_services: Vec<String>,
    /// Sites that use this version (PHP) and must be changed or removed first.
    pub blocking_sites: Vec<String>,
    /// Projects whose pin to this version will be cleared (they follow the default again).
    pub pins_cleared: Vec<String>,
    pub clears_global_default: bool,
    /// Devluma-created PHP FastCGI backends for this version that will be removed.
    pub backends_removed: usize,
    /// What stays, in plain language.
    pub kept: Vec<String>,
    pub can_uninstall: bool,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, msg, false)
}

pub fn preview(db: &Database, id: &str) -> Result<UninstallPlan, IpcError> {
    Ok(db.read(|c| {
        let inst = installations::get(c, id)?;
        let svcs: Vec<_> = services::list(c)?
            .into_iter()
            .filter(|s| s.package_installation_id == id)
            .collect();
        let (backends, blocking): (Vec<_>, Vec<_>) =
            svcs.into_iter().partition(|s| s.service_type == "fastcgi");
        let mut blocking_services: Vec<String> =
            blocking.iter().map(|s| s.instance_name.clone()).collect();
        // A running backend is in use by a site right now.
        blocking_services.extend(
            backends
                .iter()
                .filter(|s| s.observed_state.is_live())
                .map(|s| format!("{} (PHP backend, running)", s.instance_name)),
        );
        let blocking_sites: Vec<String> = sites::list(c)?
            .into_iter()
            .filter(|s| s.php_installation_id.as_deref() == Some(id))
            .map(|s| s.domain)
            .collect();
        let mut pins_cleared = Vec::new();
        for p in projects::list(c)? {
            if projects::list_pins(c, &p.id)?
                .iter()
                .any(|pin| pin.package_installation_id == id)
            {
                pins_cleared.push(p.name);
            }
        }
        let version = inst
            .package_version_id
            .split('@')
            .nth(1)
            .unwrap_or(&inst.package_version_id)
            .to_owned();
        let can_uninstall = blocking_services.is_empty()
            && blocking_sites.is_empty()
            && matches!(
                inst.status,
                InstallStatus::Installed | InstallStatus::Broken
            );
        Ok(UninstallPlan {
            installation_id: inst.id.clone(),
            package_id: inst.package_id.clone(),
            version,
            install_path: inst.install_path.clone(),
            blocking_services,
            blocking_sites,
            pins_cleared,
            clears_global_default: inst.is_global_default,
            backends_removed: backends.len(),
            kept: vec![
                "Database data folders".into(),
                "Your projects and their files".into(),
                "Backups, certificates and logs".into(),
            ],
            can_uninstall,
        })
    })?)
}

/// Removes the version. Synchronous and journaled; the folder is deleted only after the
/// database no longer points at it, and only if it is inside `packages_root`.
pub fn uninstall(
    db: &Database,
    sink: &dyn ProgressSink,
    packages_root: &Path,
    id: &str,
) -> Result<Operation, IpcError> {
    let plan = preview(db, id)?;
    if !plan.can_uninstall {
        let mut why = Vec::new();
        if !plan.blocking_services.is_empty() {
            why.push(format!(
                "remove these services first: {}",
                plan.blocking_services.join(", ")
            ));
        }
        if !plan.blocking_sites.is_empty() {
            why.push(format!(
                "these sites use it: {}",
                plan.blocking_sites.join(", ")
            ));
        }
        if why.is_empty() {
            why.push("it isn't installed".into());
        }
        return Err(invalid(format!("Can't uninstall: {}.", why.join("; "))));
    }
    let folder = PathBuf::from(&plan.install_path);
    if folder.exists() && pathpolicy::ensure_within(packages_root, &folder).is_err() {
        return Err(invalid(
            "This installation's folder is outside Devluma's packages folder; it won't be deleted.",
        ));
    }

    let journal = Journal::new(db, sink);
    let op = journal.start(&NewOperation {
        operation_type: OPERATION_TYPE,
        target_type: Some("package_installation"),
        target_id: Some(id),
    })?;
    let _ = journal.running(&op.id);
    // Intent first: pins, default and backends go; the row says "uninstalling".
    let step = db.write(|c| {
        for p in projects::list(c)? {
            if projects::list_pins(c, &p.id)?
                .iter()
                .any(|pin| pin.package_installation_id == id)
            {
                projects::unpin_runtime(c, &p.id, &plan.package_id)?;
            }
        }
        for s in services::list(c)?
            .into_iter()
            .filter(|s| s.package_installation_id == id && s.service_type == "fastcgi")
        {
            services::delete(c, &s.id)?;
        }
        installations::set_status(c, id, InstallStatus::Uninstalling)
    });
    if let Err(e) = step {
        let e = IpcError::from(e);
        let _ = journal.fail(&op.id, e.code, &e.message);
        return Err(e);
    }
    let _ = journal.progress(&op.id, "Deleting the package folder", None, None);
    if folder.exists()
        && let Err(e) = std::fs::remove_dir_all(&folder)
    {
        // Partly deleted: the row becomes `broken` so it can be reinstalled or retried.
        let _ = db.write(|c| installations::set_status(c, id, InstallStatus::Broken));
        let msg = format!(
            "Could not delete {} ({e}). Close programs using files in it and try again.",
            folder.display()
        );
        let _ = journal.fail(&op.id, ErrorCode::Internal, &msg);
        return Err(IpcError::new(ErrorCode::Internal, msg, true));
    }
    db.write(|c| installations::set_status(c, id, InstallStatus::Removed))?;
    Ok(journal.succeed(&op.id)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::Architecture;
    use crate::persistence::projects::ProjectFields;
    use crate::persistence::services::NewService;
    use crate::persistence::test_support::db;
    use crate::services::journal::test_support::RecordingSink;

    /// An installed version with a real folder under `packages/`.
    fn installed(db: &Database, packages: &Path, pkg: &str) -> String {
        use crate::domain::catalog::{ValidationOptions, fixtures, parse_and_validate};
        let catalog = parse_and_validate(
            &fixtures::document(vec![fixtures::package(
                pkg,
                vec![fixtures::version("1.0.0")],
            )]),
            ValidationOptions::default(),
        )
        .unwrap();
        let folder = packages.join(pkg).join("1.0.0-x64");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("tool.exe"), b"x").unwrap();
        let p = catalog.packages[0].clone();
        let key = crate::domain::catalog::version_row_id(pkg, "1.0.0", Architecture::X64);
        db.write(|c| {
            crate::services::install::persist_catalog_rows(c, "test", &p, &p.versions[0])?;
            let i = installations::begin(c, &key, &folder.to_string_lossy())?;
            installations::mark_installed(c, &i.id, "ok")?;
            installations::set_global_default(c, &i.id)?;
            Ok(i.id)
        })
        .unwrap()
    }

    #[test]
    fn uninstall_deletes_only_the_package_folder_and_clears_selections() {
        let dir = tempfile::tempdir().unwrap();
        let packages = dir.path().join("packages");
        let data = dir.path().join("data").join("db1");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("ibdata1"), b"precious").unwrap();
        let db = db();
        let id = installed(&db, &packages, "tool");
        let project = db
            .write(|c| {
                let p = projects::create(
                    c,
                    &ProjectFields {
                        name: "Shop".into(),
                        root_path: dir.path().display().to_string(),
                        ..Default::default()
                    },
                )?;
                projects::pin_runtime(c, &p.id, &id)?;
                Ok(p)
            })
            .unwrap();

        // A service using it blocks the uninstall.
        let svc = db
            .write(|c| {
                services::create(
                    c,
                    &NewService {
                        package_installation_id: &id,
                        instance_name: "db",
                        service_type: "database",
                        lifecycle_adapter: "mariadb",
                        port: Some(3306),
                        data_path: Some(&data.to_string_lossy()),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let plan = preview(&db, &id).unwrap();
        assert!(!plan.can_uninstall && plan.blocking_services == ["db"]);
        assert!(uninstall(&db, &RecordingSink::default(), &packages, &id).is_err());
        assert!(Path::new(&plan.install_path).is_dir(), "nothing deleted");

        // Removing the service (which keeps data) unblocks it.
        db.write(|c| services::delete(c, &svc.id)).unwrap();
        let plan = preview(&db, &id).unwrap();
        assert!(plan.can_uninstall && plan.clears_global_default);
        assert_eq!(plan.pins_cleared, ["Shop"]);
        uninstall(&db, &RecordingSink::default(), &packages, &id).unwrap();

        assert!(
            !Path::new(&plan.install_path).exists(),
            "package folder deleted"
        );
        assert_eq!(
            std::fs::read(data.join("ibdata1")).unwrap(),
            b"precious",
            "data kept"
        );
        let inst = db.read(|c| installations::get(c, &id)).unwrap();
        assert_eq!(inst.status, InstallStatus::Removed);
        assert!(!inst.is_global_default);
        assert!(
            db.read(|c| projects::list_pins(c, &project.id))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn refuses_folders_outside_packages() {
        let dir = tempfile::tempdir().unwrap();
        let db = db();
        let id = installed(&db, &dir.path().join("elsewhere"), "tool");
        let err = uninstall(
            &db,
            &RecordingSink::default(),
            &dir.path().join("packages"),
            &id,
        )
        .unwrap_err();
        assert!(err.message.contains("outside"), "{}", err.message);
        assert!(
            dir.path()
                .join("elsewhere")
                .join("tool")
                .join("1.0.0-x64")
                .is_dir()
        );
    }
}
