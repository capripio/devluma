//! Local sites (`sites`). Domains are stored normalized; uniqueness is enforced in the
//! service layer's write transaction (schema.md §3).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{DbError, DbResult, now};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: String,
    pub project_id: String,
    pub domain: String,
    pub document_root: String,
    pub web_server_service_id: String,
    pub php_installation_id: Option<String>,
    pub http_port: i64,
    pub https_port: Option<i64>,
    pub ssl_enabled: bool,
    pub config_path: String,
    pub config_hash: Option<String>,
    pub status: String,
    pub hosts_managed: bool,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "id, project_id, domain, document_root, web_server_service_id, php_installation_id, \
     http_port, config_path, config_hash, status, hosts_managed, created_at, updated_at, https_port, ssl_enabled";

fn row(r: &Row<'_>) -> rusqlite::Result<Site> {
    Ok(Site {
        id: r.get(0)?,
        project_id: r.get(1)?,
        domain: r.get(2)?,
        document_root: r.get(3)?,
        web_server_service_id: r.get(4)?,
        php_installation_id: r.get(5)?,
        http_port: r.get(6)?,
        config_path: r.get(7)?,
        config_hash: r.get(8)?,
        status: r.get(9)?,
        hosts_managed: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        https_port: r.get(13)?,
        ssl_enabled: r.get(14)?,
    })
}

/// Inserts or updates a site (by id). `created_at` is kept on update.
pub fn upsert(conn: &Connection, s: &Site) -> DbResult<Site> {
    let ts = now();
    conn.execute(
        "INSERT INTO sites (id, project_id, domain, document_root, web_server_service_id, php_installation_id,
                            http_port, ssl_enabled, config_path, config_hash, status, hosts_managed,
                            created_at, updated_at, https_port)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?13, ?8, ?9, ?10, ?11, ?12, ?12, ?14)
         ON CONFLICT (id) DO UPDATE SET
           project_id = excluded.project_id, domain = excluded.domain,
           document_root = excluded.document_root, web_server_service_id = excluded.web_server_service_id,
           php_installation_id = excluded.php_installation_id, http_port = excluded.http_port,
           config_path = excluded.config_path, config_hash = excluded.config_hash,
           status = excluded.status, hosts_managed = excluded.hosts_managed, updated_at = excluded.updated_at,
           ssl_enabled = excluded.ssl_enabled, https_port = excluded.https_port",
        params![
            s.id,
            s.project_id,
            s.domain,
            s.document_root,
            s.web_server_service_id,
            s.php_installation_id,
            s.http_port,
            s.config_path,
            s.config_hash,
            s.status,
            s.hosts_managed,
            ts,
            s.ssl_enabled,
            s.https_port
        ],
    )?;
    get(conn, &s.id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Site> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM sites WHERE id = ?1"),
        [id],
        row,
    )
    .optional()?
    .ok_or(DbError::NotFound { entity: "site" })
}

pub fn list(conn: &Connection) -> DbResult<Vec<Site>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM sites ORDER BY domain"))?;
    Ok(stmt.query_map([], row)?.collect::<Result<_, _>>()?)
}

pub fn find_by_domain(conn: &Connection, domain: &str) -> DbResult<Option<Site>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM sites WHERE domain = ?1"),
            [domain],
            row,
        )
        .optional()?)
}

pub fn delete(conn: &Connection, id: &str) -> DbResult<()> {
    if conn.execute("DELETE FROM sites WHERE id = ?1", [id])? == 0 {
        return Err(DbError::NotFound { entity: "site" });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::persistence::Database;

    /// A bare site row for services that only need its domain and port. Foreign keys are
    /// switched off for this one insert (no web server or installation rows exist); that
    /// pragma is ignored inside a transaction, hence the plain connection from `read`.
    pub fn insert(db: &Database, project_id: &str, domain: &str, http_port: i64) -> String {
        let id = crate::persistence::new_id();
        db.read(|c| {
            c.execute_batch("PRAGMA foreign_keys = OFF")?;
            let r = upsert(
                c,
                &Site {
                    id: id.clone(),
                    project_id: project_id.to_owned(),
                    domain: domain.to_owned(),
                    document_root: String::new(),
                    web_server_service_id: "none".into(),
                    php_installation_id: None,
                    http_port,
                    https_port: None,
                    ssl_enabled: false,
                    config_path: String::new(),
                    config_hash: None,
                    status: "active".into(),
                    hosts_managed: false,
                    created_at: String::new(),
                    updated_at: String::new(),
                },
            );
            c.execute_batch("PRAGMA foreign_keys = ON")?;
            r.map(|_| ())
        })
        .unwrap();
        id
    }
}
