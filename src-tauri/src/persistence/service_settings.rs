//! Simple settings for configs Devluma generates (`service_settings`, ADR 0018). A missing
//! key means the tool's default. Rows go with their service.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use super::{DbResult, now};

pub fn get(conn: &Connection, service_id: &str) -> DbResult<BTreeMap<String, String>> {
    let mut stmt = conn.prepare("SELECT key, value FROM service_settings WHERE service_id = ?1")?;
    let rows = stmt
        .query_map(params![service_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Applies changes: `Some` sets a key, `None` removes it (back to the default).
pub fn update(
    conn: &Connection,
    service_id: &str,
    changes: &BTreeMap<String, Option<String>>,
) -> DbResult<()> {
    let at = now();
    for (key, value) in changes {
        match value {
            Some(v) => conn.execute(
                "INSERT INTO service_settings (service_id, key, value, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (service_id, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![service_id, key, v, at],
            )?,
            None => conn.execute(
                "DELETE FROM service_settings WHERE service_id = ?1 AND key = ?2",
                params![service_id, key],
            )?,
        };
    }
    Ok(())
}

/// Replaces all settings (used to put the previous ones back after a failed check).
pub fn replace(
    conn: &Connection,
    service_id: &str,
    values: &BTreeMap<String, String>,
) -> DbResult<()> {
    conn.execute(
        "DELETE FROM service_settings WHERE service_id = ?1",
        params![service_id],
    )?;
    let changes = values
        .iter()
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect();
    update(conn, service_id, &changes)
}
