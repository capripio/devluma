//! Hosts-file backups and the changes applied with them (`hosts_backups`, `hosts_changes`).

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::{DbError, DbResult, new_id, now};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsBackup {
    pub id: String,
    pub original_path: String,
    pub backup_path: String,
    pub sha256_before: String,
    pub sha256_after: Option<String>,
    pub reason: String,
    pub operation_id: Option<String>,
    pub created_at: String,
    pub changes: Vec<HostsChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsChange {
    pub domain: String,
    pub ip_address: String,
    pub action: String,
}

pub struct NewBackup<'a> {
    pub original_path: &'a str,
    pub backup_path: &'a str,
    pub sha256_before: &'a str,
    pub sha256_after: &'a str,
    pub reason: &'a str,
    pub operation_id: Option<&'a str>,
}

/// Records a verified write: the backup taken before it and each entry it changed.
pub fn record(conn: &Connection, b: &NewBackup<'_>, changes: &[HostsChange]) -> DbResult<String> {
    let id = new_id();
    let ts = now();
    conn.execute(
        "INSERT INTO hosts_backups (id, original_path, backup_path, sha256_before, sha256_after, reason,
                                    operation_id, created_at, verified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            id,
            b.original_path,
            b.backup_path,
            b.sha256_before,
            b.sha256_after,
            b.reason,
            b.operation_id,
            ts
        ],
    )?;
    for c in changes {
        conn.execute(
            "INSERT INTO hosts_changes (id, backup_id, domain, ip_address, action, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'applied', ?6)",
            params![new_id(), id, c.domain, c.ip_address, c.action, ts],
        )?;
    }
    Ok(id)
}

fn changes_for(conn: &Connection, backup_id: &str) -> DbResult<Vec<HostsChange>> {
    let mut stmt = conn.prepare(
        "SELECT domain, ip_address, action FROM hosts_changes WHERE backup_id = ?1 ORDER BY domain",
    )?;
    Ok(stmt
        .query_map([backup_id], |r| {
            Ok(HostsChange {
                domain: r.get(0)?,
                ip_address: r.get(1)?,
                action: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

const COLUMNS: &str =
    "id, original_path, backup_path, sha256_before, sha256_after, reason, operation_id, created_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HostsBackup> {
    Ok(HostsBackup {
        id: r.get(0)?,
        original_path: r.get(1)?,
        backup_path: r.get(2)?,
        sha256_before: r.get(3)?,
        sha256_after: r.get(4)?,
        reason: r.get(5)?,
        operation_id: r.get(6)?,
        created_at: r.get(7)?,
        changes: Vec::new(),
    })
}

pub fn list(conn: &Connection, limit: u32) -> DbResult<Vec<HostsBackup>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM hosts_backups ORDER BY created_at DESC, id DESC LIMIT ?1"
    ))?;
    let mut out: Vec<HostsBackup> = stmt.query_map([limit], row)?.collect::<Result<_, _>>()?;
    for b in &mut out {
        b.changes = changes_for(conn, &b.id)?;
    }
    Ok(out)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<HostsBackup> {
    let mut b = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM hosts_backups WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?
        .ok_or(DbError::NotFound {
            entity: "hosts backup",
        })?;
    b.changes = changes_for(conn, id)?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    #[test]
    fn records_and_lists_backups_with_changes() {
        let db = db();
        let id = db
            .write(|c| {
                record(
                    c,
                    &NewBackup {
                        original_path: r"C:\Windows\System32\drivers\etc\hosts",
                        backup_path: r"C:\Devluma\backups\hosts\hosts-1.bak",
                        sha256_before: "aa",
                        sha256_after: "bb",
                        reason: "Site shop.test",
                        operation_id: None,
                    },
                    &[HostsChange {
                        domain: "shop.test".into(),
                        ip_address: "127.0.0.1".into(),
                        action: "add".into(),
                    }],
                )
            })
            .unwrap();
        let all = db.read(|c| list(c, 10)).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].changes[0].domain, "shop.test");
        assert_eq!(db.read(|c| get(c, &id)).unwrap().reason, "Site shop.test");
    }
}
