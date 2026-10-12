//! Key/value app settings. Non-secret values only (secrets go to the OS credential store
//! and are referenced via `secret_references`).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use super::{DbResult, now};

pub fn get<T: DeserializeOwned>(conn: &Connection, key: &str) -> DbResult<Option<T>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match raw {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    })
}

pub fn set<T: Serialize>(conn: &Connection, key: &str, value: &T) -> DbResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        params![key, serde_json::to_string(value)?, now()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    #[test]
    fn typed_round_trip_and_overwrite() {
        let db = db();
        assert_eq!(
            db.read(|c| get::<bool>(c, "startup.autostart")).unwrap(),
            None
        );
        db.write(|c| set(c, "startup.autostart", &false)).unwrap();
        db.write(|c| set(c, "startup.autostart", &true)).unwrap();
        assert_eq!(
            db.read(|c| get::<bool>(c, "startup.autostart")).unwrap(),
            Some(true)
        );
    }
}
