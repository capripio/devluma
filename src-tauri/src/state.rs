//! App startup: open the metadata DB, migrate, reconcile. Failures don't crash the app —
//! they put it in a recovery state the UI explains (app_flow.md §3). The database file is
//! never deleted, reset or overwritten automatically.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::domain::catalog::Catalog;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::{Database, DbError};
use crate::services::certificates::CertificateService;
use crate::services::databases::DatabaseService;
use crate::services::diagnostics::{AppFacts, Diagnostics};
use crate::services::download::DownloadOptions;
use crate::services::hosts::HostsService;
use crate::services::install::{InstallContext, InstallManager, ProcessProbe};
use crate::services::journal::ProgressSink;
use crate::services::recovery::{self, RecoveryReport};
use crate::services::sites::SiteService;
use crate::services::supervisor::{StateSink, Supervisor};
use crate::services::tunnels::TunnelService;
use crate::services::updates::UpdateService;

pub const DB_FILE: &str = "app.db";
pub const BACKUPS_DIR: &str = "backups";

/// App-managed folders under the storage root (trd.md §13, ADR 0005).
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
    pub db_file: PathBuf,
    pub backups: PathBuf,
    pub packages: PathBuf,
    pub staging: PathBuf,
    /// App-managed per-service folders: generated config, logs, temp files.
    pub services: PathBuf,
    /// Default location for database data. User-owned: never deleted automatically.
    pub data: PathBuf,
}

impl DataPaths {
    pub fn under(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            db_file: root.join(DB_FILE),
            backups: root.join(BACKUPS_DIR),
            // Downloaded tools (was `packages\` before ADR 0014; `storage::relocate` renames it).
            packages: root.join("bin"),
            staging: root.join("staging"),
            services: root.join("services"),
            data: root.join("data"),
        }
    }

    pub fn create_all(&self) -> std::io::Result<()> {
        for dir in [
            &self.root,
            &self.backups,
            &self.packages,
            &self.staging,
            &self.services,
            &self.data,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Staging holds scratch files of unfinished operations only. Nothing resumes a partial
/// download, so after a restart everything in it is safe to remove.
fn clear_staging(paths: &DataPaths) -> std::io::Result<()> {
    for entry in std::fs::read_dir(&paths.staging)? {
        let path = entry?.path();
        if crate::security::paths::ensure_within(&paths.staging, &path).is_ok() {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupState {
    Ready,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupStatus {
    pub state: StartupState,
    pub data_dir: String,
    pub database_path: String,
    pub backups_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryReport>,
    /// Set when this startup migrated the schema and kept a pre-migration copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration_backup_path: Option<String>,
}

pub struct AppState {
    pub startup: StartupStatus,
    pub paths: DataPaths,
    pub catalog: Arc<Catalog>,
    pub installs: Arc<InstallManager>,
    /// `None` when the database couldn't be opened (recovery mode).
    pub supervisor: Option<Arc<Supervisor>>,
    pub hosts: Option<Arc<HostsService>>,
    pub sites: Option<Arc<SiteService>>,
    pub certificates: Option<Arc<CertificateService>>,
    pub tunnels: Option<Arc<TunnelService>>,
    pub databases: Option<Arc<DatabaseService>>,
    pub diagnostics: Option<Arc<Diagnostics>>,
    pub updates: Option<Arc<UpdateService>>,
    sink: Arc<dyn ProgressSink>,
    db: Option<Arc<Database>>,
}

impl AppState {
    pub fn supervisor(&self) -> Result<Arc<Supervisor>, IpcError> {
        self.db()?;
        self.supervisor.clone().ok_or_else(IpcError::internal)
    }

    pub fn hosts(&self) -> Result<Arc<HostsService>, IpcError> {
        self.db()?;
        self.hosts.clone().ok_or_else(IpcError::internal)
    }

    pub fn certificates(&self) -> Result<Arc<CertificateService>, IpcError> {
        self.db()?;
        self.certificates.clone().ok_or_else(IpcError::internal)
    }

    pub fn updates(&self) -> Result<Arc<UpdateService>, IpcError> {
        self.db()?;
        self.updates.clone().ok_or_else(IpcError::internal)
    }

    pub fn diagnostics(&self) -> Result<Arc<Diagnostics>, IpcError> {
        self.db()?;
        self.diagnostics.clone().ok_or_else(IpcError::internal)
    }

    pub fn databases(&self) -> Result<Arc<DatabaseService>, IpcError> {
        self.db()?;
        self.databases.clone().ok_or_else(IpcError::internal)
    }

    pub fn tunnels(&self) -> Result<Arc<TunnelService>, IpcError> {
        self.db()?;
        self.tunnels.clone().ok_or_else(IpcError::internal)
    }

    pub fn sites(&self) -> Result<Arc<SiteService>, IpcError> {
        self.db()?;
        self.sites.clone().ok_or_else(IpcError::internal)
    }

    pub fn probe(&self) -> Arc<dyn crate::services::install::ExecutableProbe> {
        Arc::new(ProcessProbe)
    }

    pub fn sink(&self) -> Arc<dyn ProgressSink> {
        Arc::clone(&self.sink)
    }

    pub fn install_context(&self) -> Result<InstallContext, IpcError> {
        let db = self.db()?;
        let timeouts = db
            .read(crate::services::preferences::load)?
            .download_timeouts;
        Ok(InstallContext {
            db,
            catalog: Arc::clone(&self.catalog),
            paths: self.paths.clone(),
            probe: Arc::new(ProcessProbe),
            sink: Arc::clone(&self.sink),
            download: DownloadOptions {
                connect_timeout: std::time::Duration::from_secs(timeouts.connect_secs.into()),
                idle_timeout: std::time::Duration::from_secs(timeouts.idle_secs.into()),
                ..DownloadOptions::default()
            },
        })
    }

    /// The database, or the recovery error when startup didn't complete.
    pub fn db(&self) -> Result<Arc<Database>, IpcError> {
        self.db.clone().ok_or_else(|| {
            self.startup.error.clone().unwrap_or_else(|| {
                IpcError::new(
                    ErrorCode::RecoveryRequired,
                    "Devluma's metadata database is unavailable.",
                    false,
                )
            })
        })
    }
}

pub fn initialize(
    data_dir: &Path,
    sink: Arc<dyn ProgressSink>,
    states: Arc<dyn StateSink>,
) -> AppState {
    let paths = DataPaths::under(data_dir);
    let mut startup = StartupStatus {
        state: StartupState::RecoveryRequired,
        data_dir: display(&paths.root),
        database_path: display(&paths.db_file),
        backups_dir: display(&paths.backups),
        error: None,
        recovery: None,
        migration_backup_path: None,
    };
    // A downloaded catalog is used only if it still verifies and is newer (ADR 0013).
    let bundled = crate::services::catalog::load();
    let catalog = Arc::new(
        crate::services::updates::stored_catalog(
            &paths.root,
            crate::services::updates::CATALOG_PUBLIC_KEY,
            &bundled,
        )
        .unwrap_or(bundled),
    );
    let base = |startup: StartupStatus, db: Option<Arc<Database>>| {
        let supervisor = db.as_ref().map(|db| {
            Supervisor::new(
                Arc::clone(db),
                Arc::clone(&catalog),
                paths.clone(),
                Arc::clone(&sink),
                Arc::clone(&states),
            )
        });
        let hosts = db.as_ref().map(|db| {
            let (hosts_path, writer) = crate::services::hosts::default_writer(&paths);
            Arc::new(HostsService::new(
                Arc::clone(db),
                paths.clone(),
                hosts_path,
                writer,
            ))
        });
        let certificates = db.as_ref().map(|db| {
            Arc::new(CertificateService::new(
                Arc::clone(db),
                paths.clone(),
                crate::platform::windows::certstore::ROOT,
            ))
        });
        let sites = match (&db, &supervisor, &hosts, &certificates) {
            (Some(db), Some(sup), Some(h), Some(certs)) => Some(SiteService::new(
                Arc::clone(db),
                paths.clone(),
                Arc::clone(sup),
                Arc::clone(h),
                Arc::clone(certs),
                Arc::clone(&sink),
            )),
            _ => None,
        };
        let tunnels = db
            .as_ref()
            .map(|db| TunnelService::new(Arc::clone(db), paths.clone(), Arc::clone(&sink)));
        let databases = match (&db, &supervisor) {
            (Some(db), Some(sup)) => Some(DatabaseService::new(
                Arc::clone(db),
                paths.clone(),
                Arc::clone(sup),
                Arc::clone(&sink),
            )),
            _ => None,
        };
        let diagnostics = db.as_ref().map(|db| {
            Diagnostics::new(
                Arc::clone(db),
                paths.clone(),
                AppFacts {
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    debug_build: cfg!(debug_assertions),
                },
            )
        });
        let updates = db.as_ref().map(|db| {
            UpdateService::new(
                Arc::clone(db),
                paths.root.clone(),
                env!("CARGO_PKG_VERSION").to_owned(),
                catalog.catalog_version.clone(),
            )
        });
        AppState {
            startup,
            paths: paths.clone(),
            catalog: Arc::clone(&catalog),
            installs: Arc::new(InstallManager::default()),
            supervisor,
            hosts,
            sites,
            certificates,
            tunnels,
            databases,
            diagnostics,
            updates,
            sink: Arc::clone(&sink),
            db,
        }
    };

    let opened = paths
        .create_all()
        .map_err(DbError::from)
        .and_then(|_| Database::open(&paths.db_file, &paths.backups))
        // Stored paths must match this folder (moved, or old layout) before anything uses them.
        .and_then(|db| crate::services::storage::relocate(&db, &paths.root).map(|_| db))
        .and_then(|db| recovery::reconcile(&db).map(|report| (db, report)))
        .map(|(db, report)| {
            // Best effort: a locked leftover file must not block startup.
            let _ = clear_staging(&paths);
            (db, report)
        });

    match opened {
        Ok((db, report)) => {
            startup.state = StartupState::Ready;
            startup.recovery = Some(report);
            startup.migration_backup_path = db.migration_backup().map(display);
            base(startup, Some(Arc::new(db)))
        }
        Err(err) => {
            // At startup any failure means "can't continue safely", so it's always RecoveryRequired.
            let message = match &err {
                DbError::ChecksumMismatch { .. }
                | DbError::NewerSchema { .. }
                | DbError::MigrationFailed { .. } => IpcError::from(&err).message,
                _ => "Devluma couldn't open its metadata database. It was left unchanged.".into(),
            };
            startup.error = Some(IpcError::new(ErrorCode::RecoveryRequired, message, false));
            base(startup, None)
        }
    }
}

fn display(p: &Path) -> String {
    PathBuf::from(p).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::operations::{self as ops, NewOperation};
    use crate::services::journal::test_support::RecordingSink;

    struct NoStates;
    impl StateSink for NoStates {
        fn emit_state(&self, _: &crate::services::supervisor::ServiceStatus) {}
    }

    fn initialize(dir: &Path) -> AppState {
        super::initialize(dir, Arc::new(RecordingSink::default()), Arc::new(NoStates))
    }

    #[test]
    fn startup_clears_leftover_staging_but_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        std::fs::create_dir_all(paths.staging.join("01OLDOP")).unwrap();
        std::fs::write(
            paths.staging.join("01OLDOP").join("download.zip"),
            b"partial",
        )
        .unwrap();
        std::fs::create_dir_all(paths.packages.join("php")).unwrap();

        let state = initialize(dir.path());
        assert_eq!(state.startup.state, StartupState::Ready);
        assert!(std::fs::read_dir(&paths.staging).unwrap().next().is_none());
        assert!(paths.packages.join("php").exists());
    }

    #[test]
    fn metadata_survives_restart_and_interrupted_work_is_visible() {
        let dir = tempfile::tempdir().unwrap();
        let first = initialize(dir.path());
        assert_eq!(first.startup.state, StartupState::Ready);
        let op_id = first
            .db()
            .unwrap()
            .write(|c| {
                let op = ops::create(
                    c,
                    &NewOperation {
                        operation_type: "install_package",
                        ..Default::default()
                    },
                )?;
                ops::transition(
                    c,
                    &op.id,
                    crate::domain::operation::OperationStatus::Running,
                    None,
                )?;
                Ok(op.id)
            })
            .unwrap();
        drop(first); // simulated crash: never finished

        let second = initialize(dir.path());
        assert_eq!(
            second
                .startup
                .recovery
                .as_ref()
                .unwrap()
                .operations_needing_recovery,
            1
        );
        let op = second.db().unwrap().read(|c| ops::get(c, &op_id)).unwrap();
        assert_eq!(
            op.status,
            crate::domain::operation::OperationStatus::NeedsRecovery
        );
    }

    #[test]
    fn unreadable_database_enters_recovery_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join(DB_FILE);
        std::fs::write(
            &db_path,
            b"this is not a sqlite database, but it is the user's file",
        )
        .unwrap();

        let state = initialize(dir.path());
        assert_eq!(state.startup.state, StartupState::RecoveryRequired);
        let err = state.startup.error.clone().unwrap();
        assert_eq!(err.code, ErrorCode::RecoveryRequired);
        assert!(state.db().is_err());
        assert_eq!(
            std::fs::read(&db_path).unwrap(),
            b"this is not a sqlite database, but it is the user's file"
        );
    }
}
