//! Database operations (app_flow.md §9, Phase 9): connection details, backups and restores.
//!
//! Two kinds of backup:
//! - **logical** — one database through the engine's own dump tool (`mariadb-dump`,
//!   `pg_dump -Fc`) while the server runs;
//! - **snapshot** — a copy of the whole data folder while the service is stopped (the only
//!   kind for MongoDB, whose download doesn't include dump tools).
//!
//! Restores never destroy data silently: the preview says exactly what will be replaced and
//! the request must confirm it. Before a logical restore replaces an existing database, an
//! automatic backup of it is taken; if the restore fails, that backup is put back. A snapshot
//! restore moves the current data folder aside (it is kept, never deleted) and moves it back
//! if copying fails.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::journal::{Journal, ProgressSink};
use super::supervisor::Supervisor;
use super::supervisor::behavior::{Failure, failure};
use crate::error::{ErrorCode, IpcError};
use crate::persistence::backups::{self, BackupRecord, NewBackup};
use crate::persistence::operations::NewOperation;
use crate::persistence::services::ServiceInstance;
use crate::persistence::{Database, DbError, installations};
use crate::platform::windows::process::{run_captured, run_captured_with_input};
use crate::state::DataPaths;

const TOOL_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mariadb,
    Postgresql,
    Mongodb,
}

impl Engine {
    pub fn of(svc: &ServiceInstance) -> Option<Self> {
        match svc.lifecycle_adapter.as_str() {
            "mariadb" => Some(Self::Mariadb),
            "postgresql" => Some(Self::Postgresql),
            "mongodb" => Some(Self::Mongodb),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mariadb => "mariadb",
            Self::Postgresql => "postgresql",
            Self::Mongodb => "mongodb",
        }
    }

    fn logical_backups(self) -> bool {
        !matches!(self, Self::Mongodb)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub url: String,
    /// Command line for the engine's own client, if the download includes one.
    pub cli: Option<String>,
    /// Honest note about the credentials (design.md: never present a default as secure).
    pub credential_note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseServiceView {
    pub service_id: String,
    pub name: String,
    pub engine: Engine,
    pub version: String,
    pub observed_state: String,
    pub health_status: String,
    pub data_path: Option<String>,
    pub data_initialized: bool,
    pub connection: Option<ConnectionInfo>,
    pub logical_backups: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub backup_id: String,
    /// Service to restore into; defaults to the one the backup came from.
    #[serde(default)]
    pub service_id: Option<String>,
    /// Logical backups: database to restore into; defaults to the backed-up database.
    #[serde(default)]
    pub target_database: Option<String>,
    /// Must be true when the preview says something will be replaced.
    #[serde(default)]
    pub confirm_overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePlan {
    pub backup: BackupRecord,
    pub service_id: String,
    pub service_name: String,
    pub target_database: Option<String>,
    /// Something existing will be replaced (the request must confirm it).
    pub overwrites: bool,
    /// Plain-language list of what happens.
    pub effects: Vec<String>,
    /// Why the restore can't run right now, if it can't.
    pub blocked: Option<String>,
}

pub struct DatabaseService {
    db: Arc<Database>,
    paths: DataPaths,
    supervisor: Arc<Supervisor>,
    progress: Arc<dyn ProgressSink>,
}

fn db_fail(e: DbError) -> Failure {
    let i = IpcError::from(e);
    failure(i.code, i.message)
}

fn invalid(msg: impl Into<String>) -> Failure {
    failure(ErrorCode::ValidationError, msg)
}

/// Database names Devluma passes to tools and SQL: letters, digits, `_`, `-`, `$`.
pub fn check_db_name(name: &str) -> Result<(), Failure> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '$'));
    if ok {
        Ok(())
    } else {
        Err(invalid(
            "Use 1–63 letters, digits, '_', '-' or '$' for the database name.",
        ))
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Copies a folder tree; refuses links (a data folder shouldn't contain any).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut total = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_symlink() {
            return Err(std::io::Error::other(format!(
                "{} is a link; Devluma doesn't copy links",
                entry.path().display()
            )));
        } else if kind.is_dir() {
            total += copy_tree(&entry.path(), &dest)?;
        } else {
            total += std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(total)
}

fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string()
}

/// Which installed tools to run for a service.
struct Tools {
    engine: Engine,
    bin: PathBuf,
    port: u16,
}

impl Tools {
    fn exe(&self, name: &str) -> PathBuf {
        self.bin.join(name)
    }

    fn port_arg(&self) -> String {
        self.port.to_string()
    }

    /// Runs one SQL statement; returns stdout lines.
    fn sql(&self, database: Option<&str>, statement: &str) -> Result<Vec<String>, Failure> {
        let port = self.port_arg();
        let (exe, args): (PathBuf, Vec<&str>) = match self.engine {
            Engine::Mariadb => {
                let mut a = vec!["--host=127.0.0.1", "--user=root", "-N", "-B"];
                a.push("--port");
                a.push(&port);
                if let Some(d) = database {
                    a.push(d);
                }
                a.extend(["-e", statement]);
                (self.exe("mariadb.exe"), a)
            }
            Engine::Postgresql => (
                self.exe("psql.exe"),
                vec![
                    "-h",
                    "127.0.0.1",
                    "-p",
                    &port,
                    "-U",
                    "postgres",
                    "-d",
                    database.unwrap_or("postgres"),
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-tA",
                    "-c",
                    statement,
                ],
            ),
            Engine::Mongodb => return Err(invalid("MongoDB has no SQL client.")),
        };
        let out = run_captured(&exe, &args, QUERY_TIMEOUT).map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not run {}: {e}", exe.display()),
            )
        })?;
        if out.exit_code != Some(0) {
            return Err(failure(
                ErrorCode::ProcessExited,
                format!(
                    "The database refused: {}",
                    out.stderr.trim().chars().take(400).collect::<String>()
                ),
            ));
        }
        Ok(out
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn list_databases(&self) -> Result<Vec<String>, Failure> {
        Ok(match self.engine {
            Engine::Mariadb => self
                .sql(None, "SHOW DATABASES")?
                .into_iter()
                .filter(|d| {
                    !matches!(
                        d.as_str(),
                        "information_schema" | "performance_schema" | "mysql" | "sys"
                    )
                })
                .collect(),
            Engine::Postgresql => self.sql(
                None,
                "SELECT datname FROM pg_database WHERE NOT datistemplate ORDER BY datname",
            )?,
            Engine::Mongodb => Vec::new(),
        })
    }

    fn quoted(&self, name: &str) -> String {
        match self.engine {
            Engine::Mariadb => format!("`{name}`"),
            _ => format!("\"{name}\""),
        }
    }

    fn dump(&self, database: &str, dest: &Path) -> Result<(), Failure> {
        let port = self.port_arg();
        let file = dest.display().to_string();
        let (exe, args): (PathBuf, Vec<&str>) = match self.engine {
            Engine::Mariadb => (
                self.exe("mariadb-dump.exe"),
                vec![
                    "--host=127.0.0.1",
                    "--user=root",
                    "--port",
                    &port,
                    "--single-transaction",
                    "--routines",
                    "--triggers",
                    "--events",
                    "--result-file",
                    &file,
                    database,
                ],
            ),
            Engine::Postgresql => (
                self.exe("pg_dump.exe"),
                vec![
                    "-h",
                    "127.0.0.1",
                    "-p",
                    &port,
                    "-U",
                    "postgres",
                    "-Fc",
                    "-f",
                    &file,
                    database,
                ],
            ),
            Engine::Mongodb => return Err(invalid("Use a data folder snapshot for MongoDB.")),
        };
        let out = run_captured(&exe, &args, TOOL_TIMEOUT).map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not run {}: {e}", exe.display()),
            )
        })?;
        if out.exit_code != Some(0) || !dest.is_file() {
            let _ = std::fs::remove_file(dest);
            return Err(failure(
                ErrorCode::ProcessExited,
                format!(
                    "The backup tool failed: {}",
                    out.stderr.trim().chars().take(400).collect::<String>()
                ),
            ));
        }
        Ok(())
    }

    /// Drops and recreates `database`, then loads `file` into it.
    fn load(&self, database: &str, file: &Path) -> Result<(), Failure> {
        let q = self.quoted(database);
        match self.engine {
            Engine::Mariadb => {
                self.sql(None, &format!("DROP DATABASE IF EXISTS {q}"))?;
                self.sql(None, &format!("CREATE DATABASE {q}"))?;
                // Fed through stdin: the client then stops at the first failing statement and
                // exits non-zero. (Verified: `-e "source <file>"` exits 0 even when the file's
                // statements fail, which would report a broken restore as a success.)
                let port = self.port_arg();
                let exe = self.exe("mariadb.exe");
                let out = run_captured_with_input(
                    &exe,
                    &["--host=127.0.0.1", "--user=root", "--port", &port, database],
                    Some(file),
                    TOOL_TIMEOUT,
                )
                .map_err(|e| {
                    failure(
                        ErrorCode::ProcessExited,
                        format!("Could not run mariadb: {e}"),
                    )
                })?;
                if out.exit_code == Some(0) {
                    Ok(())
                } else {
                    Err(failure(
                        ErrorCode::ProcessExited,
                        format!(
                            "The restore failed: {}",
                            out.stderr.trim().chars().take(400).collect::<String>()
                        ),
                    ))
                }
            }
            Engine::Postgresql => {
                self.sql(None, &format!("DROP DATABASE IF EXISTS {q} WITH (FORCE)"))?;
                self.sql(None, &format!("CREATE DATABASE {q}"))?;
                let port = self.port_arg();
                let file = file.display().to_string();
                let exe = self.exe("pg_restore.exe");
                let out = run_captured(
                    &exe,
                    &[
                        "-h",
                        "127.0.0.1",
                        "-p",
                        &port,
                        "-U",
                        "postgres",
                        "-d",
                        database,
                        "--no-owner",
                        "--exit-on-error",
                        &file,
                    ],
                    TOOL_TIMEOUT,
                )
                .map_err(|e| {
                    failure(
                        ErrorCode::ProcessExited,
                        format!("Could not run pg_restore: {e}"),
                    )
                })?;
                if out.exit_code == Some(0) {
                    Ok(())
                } else {
                    Err(failure(
                        ErrorCode::ProcessExited,
                        format!(
                            "pg_restore failed: {}",
                            out.stderr.trim().chars().take(400).collect::<String>()
                        ),
                    ))
                }
            }
            Engine::Mongodb => Err(invalid("MongoDB backups are data folder snapshots.")),
        }
    }
}

impl DatabaseService {
    pub fn new(
        db: Arc<Database>,
        paths: DataPaths,
        supervisor: Arc<Supervisor>,
        progress: Arc<dyn ProgressSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            paths,
            supervisor,
            progress,
        })
    }

    fn database_service(&self, id: &str) -> Result<(ServiceInstance, Engine), Failure> {
        let svc = self.supervisor.service(id)?;
        let engine = Engine::of(&svc).ok_or_else(|| invalid("That service isn't a database."))?;
        Ok((svc, engine))
    }

    fn install_dir(&self, svc: &ServiceInstance) -> Result<(PathBuf, String), Failure> {
        let inst = self
            .db
            .read(|c| installations::get(c, &svc.package_installation_id))
            .map_err(db_fail)?;
        let version = inst
            .package_version_id
            .split('@')
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        Ok((PathBuf::from(inst.install_path), version))
    }

    fn tools(&self, svc: &ServiceInstance, engine: Engine) -> Result<Tools, Failure> {
        let (dir, _) = self.install_dir(svc)?;
        let port = svc
            .port
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| invalid("This service has no port."))?;
        Ok(Tools {
            engine,
            bin: dir.join("bin"),
            port,
        })
    }

    fn require_running(svc: &ServiceInstance) -> Result<(), Failure> {
        if svc.observed_state.as_str() == "running" {
            Ok(())
        } else {
            Err(invalid(format!(
                "Start {} first; this needs the database server running.",
                svc.instance_name
            )))
        }
    }

    fn connection(engine: Engine, port: u16, bin: &Path) -> ConnectionInfo {
        let q = |p: PathBuf| format!("\"{}\"", p.display());
        match engine {
            Engine::Mariadb => ConnectionInfo {
                host: "127.0.0.1".into(),
                port,
                user: Some("root".into()),
                url: format!("mysql://root@127.0.0.1:{port}/"),
                cli: Some(format!("{} -h 127.0.0.1 -P {port} -u root", q(bin.join("mariadb.exe")))),
                credential_note: "root has no password. That's only acceptable because the server accepts connections from this computer alone; set a password (ALTER USER) if others use this computer.".into(),
            },
            Engine::Postgresql => ConnectionInfo {
                host: "127.0.0.1".into(),
                port,
                user: Some("postgres".into()),
                url: format!("postgresql://postgres@127.0.0.1:{port}/postgres"),
                cli: Some(format!("{} -h 127.0.0.1 -p {port} -U postgres", q(bin.join("psql.exe")))),
                credential_note: "postgres signs in without a password from this computer (trust authentication). Change pg_hba.conf and set a password if others use this computer.".into(),
            },
            Engine::Mongodb => ConnectionInfo {
                host: "127.0.0.1".into(),
                port,
                user: None,
                url: format!("mongodb://127.0.0.1:{port}/"),
                cli: None,
                credential_note: "Access control is off, so no user or password is needed. The server accepts connections from this computer only. MongoDB Shell (mongosh) is a separate download.".into(),
            },
        }
    }

    pub fn list(&self) -> Result<Vec<DatabaseServiceView>, Failure> {
        let all = self
            .db
            .read(crate::persistence::services::list)
            .map_err(db_fail)?;
        let mut out = Vec::new();
        for svc in all {
            let Some(engine) = Engine::of(&svc) else {
                continue;
            };
            let (dir, version) = self.install_dir(&svc)?;
            let beh = crate::domain::adapter::AdapterId::parse(&svc.lifecycle_adapter)
                .and_then(super::supervisor::behavior::for_adapter);
            let data_initialized = match (&svc.data_path, beh) {
                (Some(p), Some(b)) => b.is_data_initialized(Path::new(p)),
                _ => false,
            };
            out.push(DatabaseServiceView {
                service_id: svc.id.clone(),
                name: svc.instance_name.clone(),
                engine,
                version,
                observed_state: svc.observed_state.as_str().to_owned(),
                health_status: svc.health_status.clone(),
                data_path: svc.data_path.clone(),
                data_initialized,
                connection: svc
                    .port
                    .and_then(|p| u16::try_from(p).ok())
                    .map(|p| Self::connection(engine, p, &dir.join("bin"))),
                logical_backups: engine.logical_backups(),
            });
        }
        Ok(out)
    }

    /// User databases on a running server.
    pub fn databases(&self, service_id: &str) -> Result<Vec<String>, Failure> {
        let (svc, engine) = self.database_service(service_id)?;
        Self::require_running(&svc)?;
        self.tools(&svc, engine)?.list_databases()
    }

    pub fn backups(&self) -> Result<Vec<BackupRecord>, Failure> {
        self.db.read(backups::list).map_err(db_fail)
    }

    fn backup_dir(&self, service_id: &str) -> PathBuf {
        self.paths.backups.join("databases").join(service_id)
    }

    fn logical_backup(
        &self,
        svc: &ServiceInstance,
        engine: Engine,
        database: &str,
        note: Option<&str>,
        op: &str,
    ) -> Result<BackupRecord, Failure> {
        check_db_name(database)?;
        let tools = self.tools(svc, engine)?;
        if !tools.list_databases()?.iter().any(|d| d == database) {
            return Err(invalid(format!("There's no database named {database}.")));
        }
        let dir = self.backup_dir(&svc.id);
        std::fs::create_dir_all(&dir).map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let ext = if engine == Engine::Postgresql {
            "dump"
        } else {
            "sql"
        };
        let file = dir.join(format!("{database}-{}.{ext}", stamp()));
        tools.dump(database, &file)?;
        let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        let sha = sha256_file(&file).map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let path = file.display().to_string();
        self.db
            .write(|c| {
                backups::insert(
                    c,
                    &NewBackup {
                        service_instance_id: Some(&svc.id),
                        service_name: &svc.instance_name,
                        engine: engine.as_str(),
                        kind: "logical",
                        database_name: Some(database),
                        path: &path,
                        size_bytes: size,
                        sha256: Some(&sha),
                        note,
                        operation_id: Some(op),
                    },
                )
            })
            .map_err(db_fail)
    }

    /// Runs `work` as a journaled operation (synchronously; commands call it on a blocking
    /// thread).
    fn journaled<T>(
        &self,
        op_type: &str,
        target: &str,
        work: impl FnOnce(&Journal<'_>, &str) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let op = journal
            .start(&NewOperation {
                operation_type: op_type,
                target_type: Some("service_instance"),
                target_id: Some(target),
            })
            .map_err(db_fail)?;
        let _ = journal.running(&op.id);
        match work(&journal, &op.id) {
            Ok(v) => {
                let _ = journal.succeed(&op.id);
                Ok(v)
            }
            Err(f) => {
                let _ = journal.fail(&op.id, f.code, &f.message);
                Err(f)
            }
        }
    }

    /// Logical backup of one database (server must be running).
    pub fn backup_database(
        &self,
        service_id: &str,
        database: &str,
    ) -> Result<BackupRecord, Failure> {
        let (svc, engine) = self.database_service(service_id)?;
        if !engine.logical_backups() {
            return Err(invalid(
                "MongoDB backups are data folder snapshots; stop the service and take a snapshot.",
            ));
        }
        Self::require_running(&svc)?;
        self.journaled("backup_database", service_id, |j, op| {
            let _ = j.progress(op, &format!("Backing up {database}"), None, None);
            self.logical_backup(&svc, engine, database, None, op)
        })
    }

    /// Copy of the whole data folder (service must be stopped, so the files are consistent).
    pub fn snapshot(&self, service_id: &str) -> Result<BackupRecord, Failure> {
        let (svc, engine) = self.database_service(service_id)?;
        if svc.observed_state.is_live() {
            return Err(invalid(
                "Stop the service first, so the data files are consistent.",
            ));
        }
        let data = svc
            .data_path
            .clone()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .ok_or_else(|| invalid("This service's data folder doesn't exist."))?;
        self.journaled("snapshot_database", service_id, |j, op| {
            let _ = j.progress(op, "Copying the data folder", None, None);
            let dest = self
                .backup_dir(&svc.id)
                .join(format!("snapshot-{}", stamp()));
            let size = copy_tree(&data, &dest).map_err(|e| {
                let _ = std::fs::remove_dir_all(&dest);
                failure(
                    ErrorCode::Internal,
                    format!("Could not copy the data folder: {e}"),
                )
            })?;
            let path = dest.display().to_string();
            self.db
                .write(|c| {
                    backups::insert(
                        c,
                        &NewBackup {
                            service_instance_id: Some(&svc.id),
                            service_name: &svc.instance_name,
                            engine: engine.as_str(),
                            kind: "snapshot",
                            path: &path,
                            size_bytes: size,
                            operation_id: Some(op),
                            ..Default::default()
                        },
                    )
                })
                .map_err(db_fail)
        })
    }

    pub fn preview_restore(&self, req: &RestoreRequest) -> Result<RestorePlan, Failure> {
        let backup = self
            .db
            .read(|c| backups::get(c, &req.backup_id))
            .map_err(db_fail)?;
        let service_id = req
            .service_id
            .clone()
            .or_else(|| backup.service_instance_id.clone())
            .ok_or_else(|| invalid("Choose the database service to restore into."))?;
        let (svc, engine) = self.database_service(&service_id)?;
        if engine.as_str() != backup.engine {
            return Err(invalid(format!(
                "This is a {} backup; {} is {}.",
                backup.engine,
                svc.instance_name,
                engine.as_str()
            )));
        }
        let present = Path::new(&backup.path).exists();
        let mut effects = Vec::new();
        let mut blocked =
            (!present).then(|| format!("The backup file {} no longer exists.", backup.path));
        let (target_database, overwrites) = if backup.kind == "logical" {
            let target = req
                .target_database
                .clone()
                .or_else(|| backup.database_name.clone())
                .unwrap_or_default();
            check_db_name(&target)?;
            let exists = if svc.observed_state.as_str() == "running" {
                self.tools(&svc, engine)?
                    .list_databases()?
                    .contains(&target)
            } else {
                blocked.get_or_insert_with(|| format!("Start {} first.", svc.instance_name));
                false
            };
            if exists {
                effects.push(format!(
                    "The database {target} on {} is replaced by the backup's contents.",
                    svc.instance_name
                ));
                effects.push(format!("First, {target} is backed up automatically; if the restore fails, that backup is put back."));
            } else {
                effects.push(format!(
                    "A new database {target} is created on {} from the backup.",
                    svc.instance_name
                ));
            }
            (Some(target), exists)
        } else {
            let data = svc.data_path.clone().unwrap_or_default();
            if svc.observed_state.is_live() {
                blocked.get_or_insert_with(|| format!("Stop {} first.", svc.instance_name));
            }
            let has_data = Path::new(&data).is_dir()
                && std::fs::read_dir(&data)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);
            if has_data {
                effects.push(format!(
                    "The current data folder {data} is moved to {data}.before-restore-<time> and kept (Devluma never deletes it)."
                ));
            }
            effects.push(format!(
                "The snapshot from {} is copied to {data}.",
                backup.created_at
            ));
            (None, has_data)
        };
        Ok(RestorePlan {
            backup,
            service_id,
            service_name: svc.instance_name,
            target_database,
            overwrites,
            effects,
            blocked,
        })
    }

    pub fn restore(&self, req: &RestoreRequest) -> Result<RestorePlan, Failure> {
        let plan = self.preview_restore(req)?;
        if let Some(b) = &plan.blocked {
            return Err(invalid(b.clone()));
        }
        if plan.overwrites && !req.confirm_overwrite {
            return Err(invalid(
                "This restore replaces existing data. Review the preview and confirm the overwrite.",
            ));
        }
        let (svc, engine) = self.database_service(&plan.service_id)?;
        self.journaled("restore_database", &plan.service_id, |j, op| {
            if plan.backup.kind == "logical" {
                self.restore_logical(&svc, engine, &plan, j, op)
            } else {
                self.restore_snapshot(&svc, &plan, j, op)
            }
        })?;
        Ok(plan)
    }

    fn restore_logical(
        &self,
        svc: &ServiceInstance,
        engine: Engine,
        plan: &RestorePlan,
        j: &Journal<'_>,
        op: &str,
    ) -> Result<(), Failure> {
        let target = plan.target_database.as_deref().unwrap_or_default();
        let tools = self.tools(svc, engine)?;
        let safety = if plan.overwrites {
            let _ = j.progress(
                op,
                &format!("Backing up {target} before replacing it"),
                None,
                None,
            );
            let note = format!("Automatic backup before restoring {}", plan.backup.id);
            Some(self.logical_backup(svc, engine, target, Some(&note), op)?)
        } else {
            None
        };
        let _ = j.progress(op, &format!("Restoring into {target}"), None, None);
        let Err(f) = tools.load(target, Path::new(&plan.backup.path)) else {
            return Ok(());
        };
        match safety {
            Some(s) => {
                let _ = j.progress(
                    op,
                    &format!("Restore failed; putting {target} back"),
                    None,
                    None,
                );
                match tools.load(target, Path::new(&s.path)) {
                    Ok(()) => Err(failure(
                        f.code,
                        format!("{} {target} was put back as it was before.", f.message),
                    )),
                    Err(f2) => Err(failure(
                        f.code,
                        format!(
                            "{} Putting {target} back also failed ({}); its backup is at {}.",
                            f.message, f2.message, s.path
                        ),
                    )),
                }
            }
            None => {
                // Nothing existed before: don't leave a half-loaded new database behind.
                let _ = tools.sql(
                    None,
                    &format!("DROP DATABASE IF EXISTS {}", tools.quoted(target)),
                );
                Err(f)
            }
        }
    }

    fn restore_snapshot(
        &self,
        svc: &ServiceInstance,
        plan: &RestorePlan,
        j: &Journal<'_>,
        op: &str,
    ) -> Result<(), Failure> {
        let data = PathBuf::from(svc.data_path.clone().unwrap_or_default());
        let aside = PathBuf::from(format!("{}.before-restore-{}", data.display(), stamp()));
        if data.exists() {
            std::fs::rename(&data, &aside).map_err(|e| {
                failure(
                    ErrorCode::Internal,
                    format!(
                        "Could not move the current data folder aside ({e}); nothing was changed."
                    ),
                )
            })?;
            let _ = j.progress(
                op,
                &format!("Current data kept at {}", aside.display()),
                None,
                None,
            );
        }
        let _ = j.progress(op, "Copying the snapshot", None, None);
        if let Err(e) = copy_tree(Path::new(&plan.backup.path), &data) {
            let _ = std::fs::remove_dir_all(&data);
            if aside.exists() {
                let _ = std::fs::rename(&aside, &data);
            }
            return Err(failure(
                ErrorCode::Internal,
                format!(
                    "Could not copy the snapshot ({e}); the previous data folder was put back."
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names_are_restricted() {
        for ok in ["shop", "shop_test", "app-2", "a$b"] {
            assert!(check_db_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a b", "x`; DROP", "x\"y", "a/b", &"x".repeat(64)] {
            assert!(check_db_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn copy_tree_copies_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("a").join("b")).unwrap();
        std::fs::write(src.join("top.dat"), b"12345").unwrap();
        std::fs::write(src.join("a").join("b").join("deep.dat"), b"678").unwrap();
        let n = copy_tree(&src, &dir.path().join("dst")).unwrap();
        assert_eq!(n, 8);
        assert_eq!(
            std::fs::read(dir.path().join("dst").join("a").join("b").join("deep.dat")).unwrap(),
            b"678"
        );
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::services::journal::test_support::RecordingSink;
    use crate::services::supervisor::tests::{LiveEnv, free_port};

    fn service(env: &LiveEnv) -> Arc<DatabaseService> {
        DatabaseService::new(
            Arc::clone(&env.db),
            env.paths.clone(),
            Arc::clone(&env.sup),
            Arc::new(RecordingSink::default()),
        )
    }

    fn req(backup: &BackupRecord, target: Option<&str>, confirm: bool) -> RestoreRequest {
        RestoreRequest {
            backup_id: backup.id.clone(),
            service_id: None,
            target_database: target.map(str::to_owned),
            confirm_overwrite: confirm,
        }
    }

    /// Logical backup → change data → restore (needs confirmation) → data back; a broken
    /// backup file fails the restore and the original database is put back.
    async fn logical_round_trip(package: &str, version: &str, create_table: &str) {
        let env = LiveEnv::install(&[(package, version)]).await;
        let id = env.create(package, "db", free_port(), None).await;
        env.start(&id).await;
        let dbs = service(&env);
        let (svc, engine) = dbs.database_service(&id).unwrap();
        let tools = dbs.tools(&svc, engine).unwrap();
        tools.sql(None, "CREATE DATABASE shop").unwrap();
        tools.sql(Some("shop"), create_table).unwrap();
        tools
            .sql(Some("shop"), "INSERT INTO notes VALUES ('original')")
            .unwrap();

        let view = dbs.list().unwrap();
        assert!(
            view[0]
                .connection
                .as_ref()
                .unwrap()
                .credential_note
                .contains("password")
        );
        assert!(dbs.databases(&id).unwrap().contains(&"shop".to_owned()));
        let backup = dbs.backup_database(&id, "shop").unwrap();
        assert!(backup.size_bytes > 0 && backup.sha256.is_some());

        tools.sql(Some("shop"), "DELETE FROM notes").unwrap();
        tools
            .sql(Some("shop"), "INSERT INTO notes VALUES ('changed')")
            .unwrap();

        let plan = dbs.preview_restore(&req(&backup, None, false)).unwrap();
        assert!(plan.overwrites, "{:?}", plan.effects);
        assert!(
            plan.effects
                .iter()
                .any(|e| e.contains("backed up automatically"))
        );
        assert!(
            dbs.restore(&req(&backup, None, false))
                .unwrap_err()
                .message
                .contains("confirm")
        );
        assert_eq!(
            tools.sql(Some("shop"), "SELECT body FROM notes").unwrap(),
            ["changed"],
            "unconfirmed: untouched"
        );

        dbs.restore(&req(&backup, None, true)).unwrap();
        assert_eq!(
            tools.sql(Some("shop"), "SELECT body FROM notes").unwrap(),
            ["original"]
        );
        let safety = dbs.backups().unwrap();
        assert!(safety.iter().any(|b| {
            b.note
                .as_deref()
                .is_some_and(|n| n.contains("before restoring"))
        }));

        // Restore into a new database: nothing is overwritten.
        let plan = dbs
            .preview_restore(&req(&backup, Some("shop_copy"), false))
            .unwrap();
        assert!(!plan.overwrites);
        dbs.restore(&req(&backup, Some("shop_copy"), false))
            .unwrap();
        assert_eq!(
            tools
                .sql(Some("shop_copy"), "SELECT body FROM notes")
                .unwrap(),
            ["original"]
        );

        // A corrupt backup: the restore fails and the existing database is put back.
        tools
            .sql(Some("shop"), "INSERT INTO notes VALUES ('latest')")
            .unwrap();
        let corrupt = env.paths.backups.join("corrupt.bin");
        std::fs::write(&corrupt, b"this is not a valid backup ;;; garbage").unwrap();
        let bad = env
            .db
            .write(|c| {
                backups::insert(
                    c,
                    &NewBackup {
                        service_instance_id: Some(&id),
                        service_name: "db",
                        engine: engine.as_str(),
                        kind: "logical",
                        database_name: Some("shop"),
                        path: &corrupt.to_string_lossy(),
                        size_bytes: 1,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let err = dbs.restore(&req(&bad, None, true)).unwrap_err();
        assert!(err.message.contains("put back"), "{}", err.message);
        let mut rows = tools.sql(Some("shop"), "SELECT body FROM notes").unwrap();
        rows.sort();
        assert_eq!(rows, ["latest", "original"], "original data preserved");
        env.stop_cleanly(&id, true).await;
    }

    /// `cargo test live_mariadb_backup -- --ignored` (set `DEVLUMA_LIVE_CACHE` to reuse downloads).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_mariadb_backup_and_restore() {
        logical_round_trip("mariadb", "11.8.9", "CREATE TABLE notes (body VARCHAR(50))").await;
    }

    /// `cargo test live_postgresql_backup -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_postgresql_backup_and_restore() {
        logical_round_trip("postgresql", "18.6", "CREATE TABLE notes (body text)").await;
    }

    /// `cargo test live_mongodb_snapshot -- --ignored`: snapshot while stopped; restoring it
    /// moves the current data aside (kept) and brings the snapshot back.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_mongodb_snapshot_and_restore() {
        let env = LiveEnv::install(&[("mongodb", "9.0.2")]).await;
        let port = free_port();
        let id = env.create("mongodb", "mongo", port, None).await;
        let dbs = service(&env);
        env.start(&id).await;
        assert!(
            dbs.snapshot(&id)
                .unwrap_err()
                .message
                .contains("Stop the service")
        );
        env.stop_cleanly(&id, true).await;

        let data = PathBuf::from(
            env.db
                .read(|c| crate::persistence::services::get(c, &id))
                .unwrap()
                .data_path
                .unwrap(),
        );
        let snap = dbs.snapshot(&id).unwrap();
        assert!(Path::new(&snap.path).join("WiredTiger").is_file());

        // Data changes after the snapshot (a marker file stands in for new data).
        std::fs::write(data.join("devluma-marker.txt"), b"after snapshot").unwrap();
        let plan = dbs.preview_restore(&req(&snap, None, false)).unwrap();
        assert!(plan.overwrites && plan.effects[0].contains("kept"));
        assert!(
            dbs.restore(&req(&snap, None, false)).is_err(),
            "needs confirmation"
        );
        dbs.restore(&req(&snap, None, true)).unwrap();
        assert!(
            !data.join("devluma-marker.txt").exists(),
            "snapshot contents restored"
        );
        let parent = data.parent().unwrap();
        let aside = std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().contains(".before-restore-"))
            .expect("previous data kept");
        assert!(aside.path().join("devluma-marker.txt").is_file());

        env.start(&id).await;
        assert!(
            crate::services::supervisor::tests::mongo_ping(port),
            "restored data starts"
        );
        env.stop_cleanly(&id, true).await;
    }
}
