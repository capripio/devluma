//! Records of database backups (files under `<root>/backups/databases/`).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{DbError, DbResult, new_id, now};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub id: String,
    pub service_instance_id: Option<String>,
    pub service_name: String,
    pub engine: String,
    /// `logical` (one database, made by the engine's dump tool) or `snapshot` (a copy of the
    /// whole data folder, made while the service was stopped).
    pub kind: String,
    pub database_name: Option<String>,
    pub path: String,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub note: Option<String>,
    pub operation_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewBackup<'a> {
    pub service_instance_id: Option<&'a str>,
    pub service_name: &'a str,
    pub engine: &'a str,
    pub kind: &'a str,
    pub database_name: Option<&'a str>,
    pub path: &'a str,
    pub size_bytes: u64,
    pub sha256: Option<&'a str>,
    pub note: Option<&'a str>,
    pub operation_id: Option<&'a str>,
}

const COLUMNS: &str = "id, service_instance_id, service_name, engine, kind, database_name, path, \
     size_bytes, sha256, note, operation_id, created_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<BackupRecord> {
    Ok(BackupRecord {
        id: r.get(0)?,
        service_instance_id: r.get(1)?,
        service_name: r.get(2)?,
        engine: r.get(3)?,
        kind: r.get(4)?,
        database_name: r.get(5)?,
        path: r.get(6)?,
        size_bytes: r.get(7)?,
        sha256: r.get(8)?,
        note: r.get(9)?,
        operation_id: r.get(10)?,
        created_at: r.get(11)?,
    })
}

pub fn insert(conn: &Connection, b: &NewBackup<'_>) -> DbResult<BackupRecord> {
    let id = new_id();
    conn.execute(
        &format!("INSERT INTO database_backups ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
        params![
            id,
            b.service_instance_id,
            b.service_name,
            b.engine,
            b.kind,
            b.database_name,
            b.path,
            i64::try_from(b.size_bytes).unwrap_or(i64::MAX),
            b.sha256,
            b.note,
            b.operation_id,
            now()
        ],
    )?;
    get(conn, &id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<BackupRecord> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM database_backups WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or(DbError::NotFound { entity: "backup" })
}

/// Newest first.
pub fn list(conn: &Connection) -> DbResult<Vec<BackupRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM database_backups ORDER BY created_at DESC, id DESC"
    ))?;
    Ok(stmt.query_map([], from_row)?.collect::<Result<_, _>>()?)
}
