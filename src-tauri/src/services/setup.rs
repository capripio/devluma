//! First-run setup (app_flow.md §2, ADR 0014): what the wizard shows, and the one background
//! operation that installs the chosen tools, makes runtimes the defaults and creates services.
//!
//! Every step goes through the normal paths (`InstallManager`, `runtime::apply`,
//! `Supervisor::create`) so it is verified and journaled the same way as when done by hand.
//! Steps are independent: one failure is reported and the rest continue; anything can be
//! retried later from the regular pages. Nothing here needs administrator rights.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::install::{InstallContext, InstallManager, InstallRequest};
use super::journal::{Journal, ProgressSink};
use super::runtime::{self, Scope, SwitchRequest};
use super::storage::{self, StorageChoice};
use super::supervisor::{CreateServiceRequest, Supervisor};
use crate::domain::adapter::{AdapterId, ServiceRole};
use crate::domain::catalog::{Architecture, Catalog};
use crate::domain::operation::OperationStatus;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::{self as ops, EventLevel, NewOperation};
use crate::persistence::{Database, services, settings};
use crate::platform::windows::ports;

const COMPLETED: &str = "onboarding.completed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub completed: bool,
    pub storage: StorageChoice,
    /// The folder can be changed from the wizard (release builds).
    pub can_change_storage: bool,
    pub free_bytes: Option<u64>,
    /// The Visual C++ runtime PHP and Apache need, if it's missing.
    pub missing_prerequisite: Option<String>,
    pub catalog_version: String,
    pub catalog_problems: usize,
    pub installed_packages: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupItem {
    pub package_id: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    pub items: Vec<SetupItem>,
}

pub fn status(
    db: &Database,
    catalog: &Catalog,
    storage: &StorageChoice,
    debug_build: bool,
) -> Result<SetupStatus, IpcError> {
    let (completed, installed) = db.read(|c| {
        Ok((
            settings::get::<bool>(c, COMPLETED)?.unwrap_or(false),
            installations::list(c)?
                .iter()
                .filter(|i| i.status == InstallStatus::Installed)
                .count(),
        ))
    })?;
    Ok(SetupStatus {
        completed,
        storage: storage.clone(),
        can_change_storage: !debug_build && storage.exe_dir_writable,
        free_bytes: crate::platform::windows::disk::available_bytes(&storage.root).ok(),
        missing_prerequisite: crate::services::install::missing_prerequisite(AdapterId::Php)
            .map(str::to_owned),
        catalog_version: catalog.catalog_version.clone(),
        catalog_problems: catalog.issues.len(),
        installed_packages: installed,
    })
}

pub fn complete(db: &Database) -> Result<(), IpcError> {
    Ok(db.write(|c| settings::set(c, COMPLETED, &true))?)
}

/// Shows the wizard again on the next load (Settings → Run setup again).
pub fn reopen(db: &Database) -> Result<(), IpcError> {
    Ok(db.write(|c| settings::set(c, COMPLETED, &false))?)
}

/// Chooses the storage folder for the next start. `None` = next to devluma.exe (portable).
/// Existing data is not moved; the wizard says so.
pub fn choose_storage(
    storage: &StorageChoice,
    debug_build: bool,
    folder: Option<&str>,
) -> Result<bool, IpcError> {
    let invalid = |m: &str| IpcError::new(ErrorCode::ValidationError, m, false);
    if debug_build {
        return Err(invalid(
            "Development builds use the DEVLUMA_ROOT environment variable instead.",
        ));
    }
    let exe = storage
        .exe_dir
        .clone()
        .ok_or_else(|| invalid("Devluma's own folder is unknown."))?;
    if !storage.exe_dir_writable {
        return Err(invalid(
            "Devluma's folder can't be written to, so the choice can't be saved there.",
        ));
    }
    let target = match folder.map(str::trim).filter(|f| !f.is_empty()) {
        None => None,
        Some(f) => {
            let p = std::path::PathBuf::from(f);
            if !p.is_absolute() {
                return Err(invalid("Enter the full path of a folder."));
            }
            std::fs::create_dir_all(&p)
                .map_err(|e| invalid(&format!("Could not create that folder: {e}")))?;
            if !storage::is_writable(&p) {
                return Err(invalid("Devluma can't write to that folder."));
            }
            Some(p)
        }
    };
    let changes = target.as_deref() != Some(storage.root.as_path())
        && !(target.is_none() && storage.mode == storage::StorageMode::Portable);
    storage::write_pointer(&exe, target.as_deref()).map_err(|e| {
        IpcError::new(
            ErrorCode::Internal,
            format!("Could not save the choice: {e}"),
            true,
        )
    })?;
    Ok(changes)
}

/// First free port from `start`, not assigned to a service and not in use.
fn free_port(db: &Database, start: u16) -> Option<u16> {
    let taken: Vec<i64> = db
        .read(services::list)
        .ok()?
        .iter()
        .flat_map(|s| [s.port, s.admin_port])
        .flatten()
        .collect();
    (start..start.saturating_add(200))
        .find(|p| !taken.contains(&i64::from(*p)) && ports::is_free(*p))
}

/// Service name and ports (from Settings → default ports) for a service adapter.
fn defaults(db: &Database, adapter: AdapterId) -> Option<(&'static str, u16, Option<u16>)> {
    let name = match adapter {
        AdapterId::Nginx => "web",
        AdapterId::Apache => "apache",
        AdapterId::Mariadb => "db",
        AdapterId::Postgresql => "pg",
        AdapterId::Mongodb => "mongo",
        AdapterId::Mailpit => "mail",
        _ => return None,
    };
    let ports = db
        .read(super::preferences::load)
        .map(|p| p.default_ports)
        .unwrap_or_default();
    let (port, admin) = ports.for_adapter(adapter)?;
    Some((name, port, admin))
}

pub struct SetupDeps {
    pub db: Arc<Database>,
    pub catalog: Arc<Catalog>,
    pub install_ctx: InstallContext,
    pub installs: Arc<InstallManager>,
    pub supervisor: Arc<Supervisor>,
    pub probe: Arc<dyn super::install::ExecutableProbe>,
    pub sink: Arc<dyn ProgressSink>,
}

/// Starts the setup as one journaled operation; returns its id immediately.
pub fn start(
    deps: SetupDeps,
    plan: SetupPlan,
) -> Result<(String, tauri::async_runtime::JoinHandle<()>), IpcError> {
    if plan.items.is_empty() {
        return Err(IpcError::new(
            ErrorCode::ValidationError,
            "Choose at least one package.",
            false,
        ));
    }
    for item in &plan.items {
        deps.catalog
            .find_version(&item.package_id, &item.version, Architecture::X64)
            .ok_or_else(|| {
                IpcError::new(
                    ErrorCode::ValidationError,
                    format!("{} {} isn't in the catalog.", item.package_id, item.version),
                    false,
                )
            })?;
    }
    let journal = Journal::new(&deps.db, deps.sink.as_ref());
    let op = journal.start(&NewOperation {
        operation_type: "first_run_setup",
        target_type: None,
        target_id: None,
    })?;
    let op_id = op.id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let journal = Journal::new(&deps.db, deps.sink.as_ref());
        let _ = journal.running(&op_id);
        let total = plan.items.len() as i64;
        let mut failures: Vec<String> = Vec::new();

        for (i, item) in plan.items.iter().enumerate() {
            let pkg = deps.catalog.package(&item.package_id);
            let name = pkg.map_or(item.package_id.clone(), |p| p.manifest.display_name.clone());
            let label = format!("{name} {}", item.version);
            let _ = journal.progress(
                &op_id,
                &format!("Installing {label}"),
                Some(i as i64),
                Some(total),
            );

            // 1. Install (skipped if already installed).
            let key = crate::domain::catalog::version_row_id(
                &item.package_id,
                &item.version,
                Architecture::X64,
            );
            let already = deps
                .db
                .read(installations::list)
                .unwrap_or_default()
                .into_iter()
                .find(|i| i.package_version_id == key && i.status == InstallStatus::Installed);
            let installation = match already {
                Some(i) => Some(i),
                None => {
                    let req = InstallRequest {
                        package_id: item.package_id.clone(),
                        version: item.version.clone(),
                        architecture: Architecture::X64,
                    };
                    match deps.installs.start(deps.install_ctx.clone(), req) {
                        Ok((started, h)) => {
                            let _ = h.await;
                            let done = deps.db.read(|c| ops::get(c, &started.operation_id)).ok();
                            match done {
                                Some(o) if o.status == OperationStatus::Succeeded => deps
                                    .db
                                    .read(installations::list)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .find(|i| {
                                        i.package_version_id == key
                                            && i.status == InstallStatus::Installed
                                    }),
                                Some(o) => {
                                    failures.push(format!(
                                        "{label}: {}",
                                        o.safe_error_message
                                            .unwrap_or_else(|| "install failed".into())
                                    ));
                                    None
                                }
                                None => None,
                            }
                        }
                        Err(e) => {
                            failures.push(format!("{label}: {}", e.message));
                            None
                        }
                    }
                }
            };
            let Some(inst) = installation else { continue };
            let Some(adapter) = pkg.map(|p| p.adapter) else {
                continue;
            };

            // 2. Runtimes become the default if none is set yet.
            // Cmder is a terminal, not a runtime: it never becomes a "default".
            let is_runtime = matches!(adapter.service_role(), None | Some(ServiceRole::FastCgi))
                && adapter != AdapterId::Cmder;
            if is_runtime && runtime::effective_default_missing(&deps.db, &item.package_id) {
                let (db, catalog, probe, sink) = (
                    Arc::clone(&deps.db),
                    Arc::clone(&deps.catalog),
                    Arc::clone(&deps.probe),
                    Arc::clone(&deps.sink),
                );
                let req = SwitchRequest {
                    package_id: item.package_id.clone(),
                    scope: Scope::Global,
                    project_id: None,
                    installation_id: Some(inst.id.clone()),
                };
                let r = tauri::async_runtime::spawn_blocking(move || {
                    runtime::apply(&db, &catalog, &probe, sink.as_ref(), &req)
                })
                .await;
                if !matches!(r, Ok(Ok(_))) {
                    failures.push(format!("{label}: couldn't make it the default"));
                }
            }

            // 3. Services get one instance on a free port, unless one exists already.
            if let Some((svc_name, port, admin)) = defaults(&deps.db, adapter) {
                let exists = deps
                    .db
                    .read(services::list)
                    .unwrap_or_default()
                    .iter()
                    .any(|s| s.package_installation_id == inst.id);
                if !exists {
                    let (Some(port), admin) = (
                        free_port(&deps.db, port),
                        admin.and_then(|a| free_port(&deps.db, a)),
                    ) else {
                        failures.push(format!("{label}: no free port found"));
                        continue;
                    };
                    let _ = journal.progress(
                        &op_id,
                        &format!("Creating the {name} service on port {port}"),
                        Some(i as i64),
                        Some(total),
                    );
                    let req = CreateServiceRequest {
                        installation_id: inst.id.clone(),
                        name: svc_name.into(),
                        port,
                        admin_port: admin,
                        data_path: None,
                        use_existing_data: false,
                    };
                    match deps.supervisor.create(req) {
                        Ok((created, h)) => {
                            let _ = h.await;
                            if let Ok(o) = deps.db.read(|c| ops::get(c, &created))
                                && o.status != OperationStatus::Succeeded
                            {
                                failures.push(format!(
                                    "{label} service: {}",
                                    o.safe_error_message.unwrap_or_default()
                                ));
                            }
                        }
                        Err(e) => failures.push(format!("{label} service: {}", e.message)),
                    }
                }
            }
        }

        let _ = journal.progress(&op_id, "Done", Some(total), Some(total));
        if failures.is_empty() {
            let _ = journal.succeed(&op_id);
        } else {
            for f in &failures {
                let _ = journal.event(&op_id, EventLevel::Warn, f, &serde_json::json!({}));
            }
            let _ = journal.fail(
                &op_id,
                ErrorCode::Internal,
                &format!("{} step(s) didn't finish: {}. Everything else is set up; retry these from Runtimes & Packages or Services.", failures.len(), failures.join("; ")),
            );
        }
    });
    Ok((op.id, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    #[test]
    fn storage_choice_is_refused_in_debug_and_for_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let choice = storage::resolve_for(Some(dir.path().to_path_buf()), true);
        assert!(
            choose_storage(&choice, true, None).is_err(),
            "debug builds use DEVLUMA_ROOT"
        );
        assert!(choose_storage(&choice, false, Some(r"relative\x")).is_err());
        let other = dir.path().join("elsewhere");
        assert!(
            choose_storage(&choice, false, Some(&other.to_string_lossy())).unwrap(),
            "restart needed"
        );
        assert_eq!(storage::read_pointer(dir.path()), Some(other));
        assert!(
            !choose_storage(&choice, false, None).unwrap(),
            "back to portable: already there"
        );
        assert_eq!(storage::read_pointer(dir.path()), None);
    }

    #[test]
    fn completion_is_remembered() {
        let db = db();
        let choice = storage::resolve_for(None, false);
        let catalog = crate::services::catalog::load();
        assert!(!status(&db, &catalog, &choice, true).unwrap().completed);
        complete(&db).unwrap();
        assert!(status(&db, &catalog, &choice, true).unwrap().completed);
    }

    /// `cargo test live_first_run_setup -- --ignored` (set `DEVLUMA_LIVE_CACHE` to reuse
    /// downloads): PHP + Nginx + MariaDB in one setup run.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_first_run_setup() {
        use crate::services::download::DownloadOptions;
        use crate::services::install::ProcessProbe;
        use crate::services::journal::test_support::RecordingSink;
        let mut catalog = crate::services::catalog::load();
        let cache = crate::services::supervisor::tests::local_cache(&mut catalog);
        let catalog = Arc::new(catalog);
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::state::DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let db = Arc::new(db());
        let sink: Arc<dyn ProgressSink> = Arc::new(RecordingSink::default());
        let sup = Supervisor::new(
            Arc::clone(&db),
            Arc::clone(&catalog),
            paths.clone(),
            Arc::clone(&sink),
            Arc::new(crate::services::supervisor::tests::RecordingStates::default()),
        );
        let _cleanup = crate::services::supervisor::StopAllOnDrop(Arc::clone(&sup));
        let deps = || SetupDeps {
            db: Arc::clone(&db),
            catalog: Arc::clone(&catalog),
            install_ctx: InstallContext {
                db: Arc::clone(&db),
                catalog: Arc::clone(&catalog),
                paths: paths.clone(),
                probe: Arc::new(ProcessProbe),
                sink: Arc::clone(&sink),
                download: DownloadOptions {
                    allow_loopback_http: cache.is_some(),
                    ..DownloadOptions::default()
                },
            },
            installs: Arc::new(InstallManager::default()),
            supervisor: Arc::clone(&sup),
            probe: Arc::new(ProcessProbe),
            sink: Arc::clone(&sink),
        };
        let plan = || SetupPlan {
            items: vec![
                SetupItem {
                    package_id: "php".into(),
                    version: "8.4.26".into(),
                },
                SetupItem {
                    package_id: "nginx".into(),
                    version: "1.30.5".into(),
                },
                SetupItem {
                    package_id: "mariadb".into(),
                    version: "11.8.9".into(),
                },
            ],
        };
        let (op, h) = start(deps(), plan()).unwrap();
        h.await.unwrap();
        let o = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            o.status,
            OperationStatus::Succeeded,
            "{:?}",
            o.safe_error_message
        );
        assert!(
            !runtime::effective_default_missing(&db, "php"),
            "PHP is the default"
        );
        let svcs = db.read(services::list).unwrap();
        let names: Vec<&str> = svcs.iter().map(|s| s.instance_name.as_str()).collect();
        assert!(names.contains(&"web") && names.contains(&"db"), "{names:?}");
        assert!(
            paths.data.join("mariadb").join("db").join("mysql").is_dir(),
            "database initialized"
        );

        // Running it again changes nothing (idempotent).
        let (op, h) = start(deps(), plan()).unwrap();
        h.await.unwrap();
        assert_eq!(
            db.read(|c| ops::get(c, &op)).unwrap().status,
            OperationStatus::Succeeded
        );
        assert_eq!(db.read(services::list).unwrap().len(), svcs.len());
    }
}
