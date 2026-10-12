//! References to secrets kept in the OS credential store. Only the store's key (e.g. a
//! Credential Manager target name) is recorded here — never the secret itself.

use rusqlite::{Connection, params};

use super::{DbResult, new_id, now};

/// Records (or keeps) a reference to `opaque_key` in `provider`.
pub fn upsert(conn: &Connection, provider: &str, opaque_key: &str, label: &str) -> DbResult<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM secret_references WHERE provider = ?1 AND opaque_key = ?2)",
        params![provider, opaque_key],
        |r| r.get(0),
    )?;
    if !exists {
        conn.execute(
            "INSERT INTO secret_references (id, provider, opaque_key, label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![new_id(), provider, opaque_key, label, now()],
        )?;
    }
    Ok(())
}

pub fn delete(conn: &Connection, opaque_key: &str) -> DbResult<()> {
    conn.execute(
        "DELETE FROM secret_references WHERE opaque_key = ?1",
        [opaque_key],
    )?;
    Ok(())
}
