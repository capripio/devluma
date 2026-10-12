//! Certificates and their site bindings. Private key bytes never enter SQLite — only paths
//! and fingerprints (schema.md §3).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{DbError, DbResult, now};

pub const KIND_CA: &str = "local_ca";
pub const KIND_SIGNED: &str = "local_ca_signed";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Certificate {
    pub id: String,
    pub common_name: String,
    pub sans: Vec<String>,
    pub certificate_path: String,
    pub private_key_path: String,
    /// `local_ca` or `local_ca_signed`.
    pub issuer_type: String,
    pub fingerprint_sha256: String,
    pub not_before: String,
    pub not_after: String,
    pub trust_store: Option<String>,
    pub trusted: bool,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "id, common_name, sans_json, certificate_path, private_key_path, issuer_type, \
     fingerprint_sha256, not_before, not_after, trust_store, trusted, created_at, updated_at";

fn row(r: &Row<'_>) -> rusqlite::Result<Certificate> {
    let sans: String = r.get(2)?;
    Ok(Certificate {
        id: r.get(0)?,
        common_name: r.get(1)?,
        sans: serde_json::from_str(&sans).unwrap_or_default(),
        certificate_path: r.get(3)?,
        private_key_path: r.get(4)?,
        issuer_type: r.get(5)?,
        fingerprint_sha256: r.get(6)?,
        not_before: r.get(7)?,
        not_after: r.get(8)?,
        trust_store: r.get(9)?,
        trusted: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

/// Inserts a certificate, or replaces its details (renewal keeps the id).
pub fn upsert(conn: &Connection, c: &Certificate) -> DbResult<Certificate> {
    let ts = now();
    conn.execute(
        "INSERT INTO certificates (id, common_name, sans_json, certificate_path, private_key_path, issuer_type,
                                   fingerprint_sha256, not_before, not_after, trust_store, trusted,
                                   created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
         ON CONFLICT (id) DO UPDATE SET
           common_name = excluded.common_name, sans_json = excluded.sans_json,
           certificate_path = excluded.certificate_path, private_key_path = excluded.private_key_path,
           fingerprint_sha256 = excluded.fingerprint_sha256, not_before = excluded.not_before,
           not_after = excluded.not_after, updated_at = excluded.updated_at",
        params![
            c.id,
            c.common_name,
            serde_json::to_string(&c.sans)?,
            c.certificate_path,
            c.private_key_path,
            c.issuer_type,
            c.fingerprint_sha256,
            c.not_before,
            c.not_after,
            c.trust_store,
            c.trusted,
            ts
        ],
    )?;
    get(conn, &c.id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Certificate> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM certificates WHERE id = ?1"),
        [id],
        row,
    )
    .optional()?
    .ok_or(DbError::NotFound {
        entity: "certificate",
    })
}

pub fn list(conn: &Connection) -> DbResult<Vec<Certificate>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM certificates ORDER BY issuer_type DESC, common_name"
    ))?;
    Ok(stmt.query_map([], row)?.collect::<Result<_, _>>()?)
}

pub fn ca(conn: &Connection) -> DbResult<Option<Certificate>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM certificates WHERE issuer_type = ?1 ORDER BY created_at LIMIT 1"),
            [KIND_CA],
            row,
        )
        .optional()?)
}

pub fn set_trust(conn: &Connection, id: &str, store: Option<&str>, trusted: bool) -> DbResult<()> {
    conn.execute(
        "UPDATE certificates SET trust_store = ?2, trusted = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, store, trusted, now()],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> DbResult<()> {
    if conn.execute("DELETE FROM certificates WHERE id = ?1", [id])? == 0 {
        return Err(DbError::NotFound {
            entity: "certificate",
        });
    }
    Ok(())
}

/// Binds a site to exactly one certificate.
pub fn bind(conn: &Connection, site_id: &str, certificate_id: &str) -> DbResult<()> {
    conn.execute(
        "DELETE FROM certificate_bindings WHERE site_id = ?1",
        [site_id],
    )?;
    conn.execute(
        "INSERT INTO certificate_bindings (site_id, certificate_id, created_at) VALUES (?1, ?2, ?3)",
        params![site_id, certificate_id, now()],
    )?;
    Ok(())
}

pub fn unbind_site(conn: &Connection, site_id: &str) -> DbResult<()> {
    conn.execute(
        "DELETE FROM certificate_bindings WHERE site_id = ?1",
        [site_id],
    )?;
    Ok(())
}

/// Site ids using a certificate.
pub fn bound_sites(conn: &Connection, certificate_id: &str) -> DbResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT site_id FROM certificate_bindings WHERE certificate_id = ?1 ORDER BY site_id",
    )?;
    Ok(stmt
        .query_map([certificate_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

pub fn for_site(conn: &Connection, site_id: &str) -> DbResult<Option<Certificate>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT certificate_id FROM certificate_bindings WHERE site_id = ?1",
            [site_id],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| get(conn, &id)).transpose()
}
