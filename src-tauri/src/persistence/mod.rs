//! SQLite metadata store (schema.md). SQLite holds metadata and file references only;
//! binaries, configs, logs, keys and database data stay on the filesystem.

pub mod backups;
pub mod certificates;
pub mod hosts;
pub mod installations;
pub mod migrations;
pub mod operations;
pub mod packages;
pub mod projects;
pub mod secrets;
pub mod services;
pub mod settings;
pub mod sites;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, Transaction};

use crate::domain::operation::OperationStatus;
use migrations::{MIGRATIONS, Migration};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("applied migration {version} does not match this build (checksum mismatch)")]
    ChecksumMismatch { version: i64 },
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    NewerSchema { found: i64, supported: i64 },
    #[error("migration {version} failed: {source}")]
    MigrationFailed {
        version: i64,
        source: rusqlite::Error,
    },
    #[error("{entity} not found")]
    NotFound { entity: &'static str },
    #[error("invalid operation transition {from} -> {to}")]
    InvalidTransition {
        from: OperationStatus,
        to: OperationStatus,
    },
    /// A backend invariant would be violated (schema.md §6). The message is safe to show.
    #[error("{0}")]
    Invariant(String),
}

pub type DbResult<T> = Result<T, DbError>;

/// UTC RFC 3339 with millisecond precision; fixed width, so text order is time order.
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// One connection behind a mutex. Callers run on blocking threads (`spawn_blocking`),
/// never on the UI/main thread.
pub struct Database {
    conn: Mutex<Connection>,
    migration_backup: Option<PathBuf>,
}

impl Database {
    /// Opens (creating if needed) the app database and applies pending migrations.
    /// Before migrating an existing database, a copy is written to `backups_dir`.
    pub fn open(path: &Path, backups_dir: &Path) -> DbResult<Self> {
        Self::open_with_migrations(path, backups_dir, MIGRATIONS)
    }

    pub fn open_with_migrations(
        path: &Path,
        backups_dir: &Path,
        migrations: &[Migration],
    ) -> DbResult<Self> {
        let mut conn = Connection::open(path)?;
        configure(&conn)?;
        let report = migrations::migrate(&mut conn, Some((path, backups_dir)), migrations)?;
        Ok(Self {
            conn: Mutex::new(conn),
            migration_backup: report.backup_path,
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> DbResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrations::migrate(&mut conn, None, MIGRATIONS)?;
        Ok(Self {
            conn: Mutex::new(conn),
            migration_backup: None,
        })
    }

    /// Copy written before this open migrated an existing database, if any.
    pub fn migration_backup(&self) -> Option<&Path> {
        self.migration_backup.as_deref()
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic mid-call leaves SQLite itself consistent (transactions roll back on drop),
        // so a poisoned lock is safe to reuse.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> DbResult<T>) -> DbResult<T> {
        f(&self.lock())
    }

    /// Runs `f` in a transaction; commits on `Ok`, rolls back on `Err`.
    pub fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> DbResult<T>) -> DbResult<T> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

fn configure(conn: &Connection) -> DbResult<()> {
    // Foreign keys are per-connection in SQLite and off by default (schema.md §5).
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

/// True for `CHECK`/`UNIQUE`/`FOREIGN KEY` failures, so callers can return a domain error.
pub fn is_constraint_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn db() -> Database {
        Database::open_in_memory().expect("in-memory db")
    }
}
