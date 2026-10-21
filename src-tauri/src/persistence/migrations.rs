//! Forward-only, checksummed migrations (schema.md §5).
//!
//! - Applied migrations are verified against this build's copy; any mismatch, or a schema
//!   newer than this build knows, fails closed (`RecoveryRequired` in the UI) and the
//!   database is left untouched.
//! - Before migrating a database that already has data, a full copy is written to the
//!   backups directory.
//! - Each migration runs in its own transaction, so a failure leaves the previous version.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::{DbError, DbResult, now};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "service_job_name",
        sql: include_str!("../../migrations/0002_service_job_name.sql"),
    },
    Migration {
        version: 3,
        name: "service_admin_port",
        sql: include_str!("../../migrations/0003_service_admin_port.sql"),
    },
    Migration {
        version: 4,
        name: "database_backups",
        sql: include_str!("../../migrations/0004_database_backups.sql"),
    },
    Migration {
        version: 5,
        name: "scheduled_jobs",
        sql: include_str!("../../migrations/0005_scheduled_jobs.sql"),
    },
];

pub fn checksum(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub applied: Vec<i64>,
    pub backup_path: Option<std::path::PathBuf>,
}

/// `file` is `(database path, backups dir)`; `None` for in-memory databases (no backup).
pub fn migrate(
    conn: &mut Connection,
    file: Option<(&Path, &Path)>,
    migrations: &[Migration],
) -> DbResult<MigrationReport> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
           version    INTEGER PRIMARY KEY,
           applied_at TEXT NOT NULL,
           checksum   TEXT NOT NULL
         );",
    )?;

    let applied: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };

    let supported = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    for (version, sum) in &applied {
        match migrations.iter().find(|m| m.version == *version) {
            None if *version > supported => {
                return Err(DbError::NewerSchema {
                    found: *version,
                    supported,
                });
            }
            None => return Err(DbError::ChecksumMismatch { version: *version }),
            Some(m) if checksum(m.sql) != *sum => {
                return Err(DbError::ChecksumMismatch { version: *version });
            }
            Some(_) => {}
        }
    }

    let current = applied.last().map(|(v, _)| *v).unwrap_or(0);
    let pending: Vec<&Migration> = migrations.iter().filter(|m| m.version > current).collect();
    let mut report = MigrationReport::default();
    if pending.is_empty() {
        return Ok(report);
    }

    if let (Some((db_path, backups_dir)), true) = (file, current > 0) {
        report.backup_path = Some(backup(conn, db_path, backups_dir, current)?);
    }

    for m in pending {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)
            .map_err(|source| DbError::MigrationFailed {
                version: m.version,
                source,
            })?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at, checksum) VALUES (?1, ?2, ?3)",
            params![m.version, now(), checksum(m.sql)],
        )?;
        tx.commit()?;
        report.applied.push(m.version);
    }
    Ok(report)
}

/// Consistent copy via `VACUUM INTO` (includes WAL contents). Never overwrites a file.
fn backup(
    conn: &Connection,
    db_path: &Path,
    backups_dir: &Path,
    from_version: i64,
) -> DbResult<std::path::PathBuf> {
    std::fs::create_dir_all(backups_dir)?;
    let stem = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("app");
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let target = backups_dir.join(format!("{stem}-v{from_version}-{stamp}.db"));
    if target.exists() {
        return Err(DbError::Invariant(format!(
            "Backup file already exists: {}",
            target.display()
        )));
    }
    let target_str = target
        .to_str()
        .ok_or_else(|| DbError::Invariant("Backup path is not valid UTF-8".into()))?;
    conn.execute("VACUUM INTO ?1", [target_str])?;
    Ok(target)
}

pub fn current_version(conn: &Connection) -> DbResult<i64> {
    Ok(conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()?
        .flatten()
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::Database;

    const V1: Migration = Migration {
        version: 1,
        name: "one",
        sql: "CREATE TABLE a (id INTEGER PRIMARY KEY);",
    };
    const V2: Migration = Migration {
        version: 2,
        name: "two",
        sql: "CREATE TABLE b (id INTEGER PRIMARY KEY);",
    };
    const V2_BROKEN: Migration = Migration {
        version: 2,
        name: "two",
        sql: "CREATE TABLE b (id INTEGER PRIMARY KEY); THIS IS NOT SQL;",
    };

    fn paths() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("app.db");
        let backups = dir.path().join("backups");
        (dir, db, backups)
    }

    fn version(db: &Database) -> i64 {
        db.read(current_version).unwrap()
    }

    #[test]
    fn real_migrations_apply_and_reopen_is_idempotent() {
        let (_dir, db_path, backups) = paths();
        let db = Database::open(&db_path, &backups).unwrap();
        assert_eq!(version(&db), MIGRATIONS.last().unwrap().version);
        drop(db);
        let db = Database::open(&db_path, &backups).unwrap();
        assert_eq!(version(&db), MIGRATIONS.last().unwrap().version);
        assert!(!backups.exists(), "no backup when nothing to migrate");
    }

    #[test]
    fn upgrading_an_existing_database_writes_a_backup_first() {
        let (_dir, db_path, backups) = paths();
        drop(Database::open_with_migrations(&db_path, &backups, &[V1]).unwrap());
        let db = Database::open_with_migrations(&db_path, &backups, &[V1, V2]).unwrap();
        assert_eq!(version(&db), 2);
        let files: Vec<_> = std::fs::read_dir(&backups).unwrap().collect();
        assert_eq!(files.len(), 1);
        let name = files[0].as_ref().unwrap().file_name();
        assert!(name.to_string_lossy().starts_with("app-v1-"));
    }

    #[test]
    fn failed_migration_rolls_back_and_keeps_previous_version() {
        let (_dir, db_path, backups) = paths();
        drop(Database::open_with_migrations(&db_path, &backups, &[V1]).unwrap());
        let err = Database::open_with_migrations(&db_path, &backups, &[V1, V2_BROKEN])
            .err()
            .expect("broken migration must fail");
        assert!(matches!(err, DbError::MigrationFailed { version: 2, .. }));

        // Still at v1, table `b` was rolled back, and the fixed migration can apply later.
        let db = Database::open_with_migrations(&db_path, &backups, &[V1]).unwrap();
        assert_eq!(version(&db), 1);
        let has_b: i64 = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = 'b'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(has_b, 0);
        drop(db);
        let db = Database::open_with_migrations(&db_path, &backups, &[V1, V2]).unwrap();
        assert_eq!(version(&db), 2);
    }

    #[test]
    fn tampered_migration_fails_closed() {
        let (_dir, db_path, backups) = paths();
        drop(Database::open_with_migrations(&db_path, &backups, &[V1]).unwrap());
        let edited = Migration {
            version: 1,
            name: "one",
            sql: "CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT);",
        };
        let err = Database::open_with_migrations(&db_path, &backups, &[edited])
            .err()
            .unwrap();
        assert!(matches!(err, DbError::ChecksumMismatch { version: 1 }));
    }

    #[test]
    fn newer_schema_fails_closed_without_changes() {
        let (_dir, db_path, backups) = paths();
        drop(Database::open_with_migrations(&db_path, &backups, &[V1, V2]).unwrap());
        let err = Database::open_with_migrations(&db_path, &backups, &[V1])
            .err()
            .unwrap();
        assert!(matches!(
            err,
            DbError::NewerSchema {
                found: 2,
                supported: 1
            }
        ));
    }
}
