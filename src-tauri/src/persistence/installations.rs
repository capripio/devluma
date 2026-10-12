//! Installed package versions (`package_installations`).
//! Invariants (schema.md §6): only verified `installed` versions can be activated, and at
//! most one global default per package — both enforced here inside the caller's transaction.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{DbError, DbResult, new_id, now};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    Installing,
    Installed,
    Broken,
    Uninstalling,
    Removed,
}

impl InstallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installing => "installing",
            Self::Installed => "installed",
            Self::Broken => "broken",
            Self::Uninstalling => "uninstalling",
            Self::Removed => "removed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            Self::Installing,
            Self::Installed,
            Self::Broken,
            Self::Uninstalling,
            Self::Removed,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub id: String,
    pub package_version_id: String,
    pub package_id: String,
    pub install_path: String,
    pub status: InstallStatus,
    pub verified_at: Option<String>,
    pub integrity_result: String,
    pub installed_at: Option<String>,
    pub is_global_default: bool,
    pub metadata: serde_json::Value,
}

const SELECT: &str = "SELECT i.id, i.package_version_id, v.package_id, i.install_path, i.status,
        i.verified_at, i.integrity_result, i.installed_at, i.is_global_default, i.metadata_json
     FROM package_installations i JOIN package_versions v ON v.id = i.package_version_id";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Installation> {
    let status: String = r.get(4)?;
    let metadata: String = r.get(9)?;
    Ok(Installation {
        id: r.get(0)?,
        package_version_id: r.get(1)?,
        package_id: r.get(2)?,
        install_path: r.get(3)?,
        status: InstallStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, status.into())
        })?,
        verified_at: r.get(5)?,
        integrity_result: r.get(6)?,
        installed_at: r.get(7)?,
        is_global_default: r.get(8)?,
        metadata: serde_json::from_str(&metadata).unwrap_or_default(),
    })
}

/// Records intent before any file is written: status `installing`, not verified.
pub fn begin(
    conn: &Connection,
    package_version_id: &str,
    install_path: &str,
) -> DbResult<Installation> {
    let id = new_id();
    conn.execute(
        "INSERT INTO package_installations (id, package_version_id, install_path, status, integrity_result)
         VALUES (?1, ?2, ?3, 'installing', 'pending')",
        params![id, package_version_id, install_path],
    )?;
    get(conn, &id)
}

/// Starts (or retries) an install of `package_version_id` without duplicating rows:
/// an existing `broken`/`removed` row for the same version is reset to `installing`.
/// Returns the row and its previous status (`None` if newly created), so a failed attempt
/// can restore it. Errors if the version is installed or an install is already underway.
pub fn begin_or_resume(
    conn: &Connection,
    package_version_id: &str,
    install_path: &str,
) -> DbResult<(Installation, Option<InstallStatus>)> {
    let existing: Option<Installation> = conn
        .query_row(
            &format!(
                "{SELECT} WHERE i.package_version_id = ?1 ORDER BY i.installed_at DESC LIMIT 1"
            ),
            [package_version_id],
            from_row,
        )
        .optional()?;
    match existing {
        None => Ok((begin(conn, package_version_id, install_path)?, None)),
        Some(i) if matches!(i.status, InstallStatus::Broken | InstallStatus::Removed) => {
            conn.execute(
                "UPDATE package_installations
                 SET status = 'installing', install_path = ?2, integrity_result = 'pending',
                     verified_at = NULL, is_global_default = 0
                 WHERE id = ?1",
                params![i.id, install_path],
            )?;
            Ok((get(conn, &i.id)?, Some(i.status)))
        }
        Some(i) => Err(DbError::Invariant(format!(
            "This version is already {}.",
            i.status.as_str()
        ))),
    }
}

/// Undoes `begin_or_resume` after a failed attempt: deletes a row created by it, or puts
/// a resumed row back to its previous status. Only acts on rows still `installing`.
pub fn abandon(conn: &Connection, id: &str, previous: Option<InstallStatus>) -> DbResult<()> {
    match previous {
        None => {
            conn.execute(
                "DELETE FROM package_installations WHERE id = ?1 AND status = 'installing'",
                [id],
            )?;
        }
        Some(status) => {
            conn.execute(
                "UPDATE package_installations SET status = ?2 WHERE id = ?1 AND status = 'installing'",
                params![id, status.as_str()],
            )?;
        }
    }
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Installation> {
    conn.query_row(&format!("{SELECT} WHERE i.id = ?1"), [id], from_row)
        .optional()?
        .ok_or(DbError::NotFound {
            entity: "installation",
        })
}

pub fn list(conn: &Connection) -> DbResult<Vec<Installation>> {
    let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY v.package_id, v.version"))?;
    Ok(stmt.query_map([], from_row)?.collect::<Result<_, _>>()?)
}

/// Commits a verified install. Only valid from `installing`.
pub fn mark_installed(
    conn: &Connection,
    id: &str,
    integrity_result: &str,
) -> DbResult<Installation> {
    let current = get(conn, id)?;
    if current.status != InstallStatus::Installing {
        return Err(DbError::Invariant(format!(
            "Installation {id} is {}, not installing",
            current.status.as_str()
        )));
    }
    let ts = now();
    conn.execute(
        "UPDATE package_installations
         SET status = 'installed', integrity_result = ?2, verified_at = ?3, installed_at = ?3
         WHERE id = ?1",
        params![id, integrity_result, ts],
    )?;
    get(conn, id)
}

pub fn set_status(conn: &Connection, id: &str, status: InstallStatus) -> DbResult<()> {
    // A non-installed version can't remain the global default.
    let n = conn.execute(
        "UPDATE package_installations
         SET status = ?2, is_global_default = CASE WHEN ?2 = 'installed' THEN is_global_default ELSE 0 END
         WHERE id = ?1",
        params![id, status.as_str()],
    )?;
    if n == 0 {
        return Err(DbError::NotFound {
            entity: "installation",
        });
    }
    Ok(())
}

/// Makes `id` the single global default for its package. Returns the previous default's id
/// so the caller can journal it for rollback.
pub fn set_global_default(conn: &Connection, id: &str) -> DbResult<Option<String>> {
    let target = get(conn, id)?;
    if target.status != InstallStatus::Installed || target.verified_at.is_none() {
        return Err(DbError::Invariant(
            "Only a verified, installed version can be made the default.".into(),
        ));
    }
    let previous: Option<String> = conn
        .query_row(
            &format!("{SELECT} WHERE v.package_id = ?1 AND i.is_global_default = 1"),
            [&target.package_id],
            from_row,
        )
        .optional()?
        .map(|i| i.id);
    conn.execute(
        "UPDATE package_installations SET is_global_default = (id = ?1)
         WHERE package_version_id IN (SELECT id FROM package_versions WHERE package_id = ?2)",
        params![id, target.package_id],
    )?;
    Ok(previous)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::persistence::packages::fixtures::seed;

    /// Seeds `pkg`/`version` and returns a verified installation of it.
    pub fn installed(
        conn: &Connection,
        pkg: &str,
        version: &str,
        path: &str,
    ) -> DbResult<Installation> {
        if super::super::packages::get(conn, pkg).is_err() {
            seed(conn, pkg, &[])?;
        }
        super::super::packages::insert_version(
            conn,
            &super::super::packages::fixtures::version(&format!("{pkg}-{version}"), pkg, version),
        )?;
        let i = begin(conn, &format!("{pkg}-{version}"), path)?;
        mark_installed(conn, &i.id, "sha256-ok")
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::installed;
    use super::*;
    use crate::persistence::packages::fixtures::seed;
    use crate::persistence::test_support::db;

    #[test]
    fn install_starts_unverified_and_commits_once() {
        let db = db();
        let i = db
            .write(|c| {
                seed(c, "php", &["8.4.1"])?;
                begin(c, "php-8.4.1", "C:\\Devluma\\packages\\php\\8.4.1")
            })
            .unwrap();
        assert_eq!(i.status, InstallStatus::Installing);
        assert!(i.verified_at.is_none());

        let i = db.write(|c| mark_installed(c, &i.id, "sha256-ok")).unwrap();
        assert_eq!(i.status, InstallStatus::Installed);
        assert!(i.verified_at.is_some());
        assert!(matches!(
            db.write(|c| mark_installed(c, &i.id, "again")),
            Err(DbError::Invariant(_))
        ));
    }

    #[test]
    fn unverified_versions_cannot_become_default() {
        let db = db();
        let err = db
            .write(|c| {
                seed(c, "php", &["8.4.1"])?;
                let i = begin(c, "php-8.4.1", "p")?;
                set_global_default(c, &i.id)
            })
            .unwrap_err();
        assert!(matches!(err, DbError::Invariant(_)));
    }

    #[test]
    fn exactly_one_global_default_per_package() {
        let db = db();
        let (a, b, node) = db
            .write(|c| {
                let a = installed(c, "php", "8.3.0", "a")?;
                let b = installed(c, "php", "8.4.1", "b")?;
                let node = installed(c, "node", "22.0.0", "n")?;
                assert_eq!(set_global_default(c, &a.id)?, None);
                set_global_default(c, &node.id)?;
                assert_eq!(set_global_default(c, &b.id)?, Some(a.id.clone()));
                Ok((a.id, b.id, node.id))
            })
            .unwrap();
        let all = db.read(list).unwrap();
        let default_of = |id: &str| all.iter().find(|i| i.id == id).unwrap().is_global_default;
        assert!(!default_of(&a));
        assert!(default_of(&b));
        assert!(default_of(&node), "other packages are unaffected");
    }

    #[test]
    fn retries_reuse_the_row_and_abandon_restores_it() {
        let db = db();
        db.write(|c| seed(c, "php", &["8.4.1"])).unwrap();

        // Fresh attempt that fails: row disappears.
        let (i, prev) = db.write(|c| begin_or_resume(c, "php-8.4.1", "p")).unwrap();
        assert_eq!(prev, None);
        db.write(|c| abandon(c, &i.id, prev)).unwrap();
        assert!(db.read(list).unwrap().is_empty());

        // Interrupted attempt left a broken row: retry resumes it, failure restores it.
        let (i, _) = db.write(|c| begin_or_resume(c, "php-8.4.1", "p")).unwrap();
        db.write(|c| set_status(c, &i.id, InstallStatus::Broken))
            .unwrap();
        let (again, prev) = db.write(|c| begin_or_resume(c, "php-8.4.1", "p")).unwrap();
        assert_eq!(
            (again.id.as_str(), prev),
            (i.id.as_str(), Some(InstallStatus::Broken))
        );
        assert!(
            matches!(
                db.write(|c| begin_or_resume(c, "php-8.4.1", "p")),
                Err(DbError::Invariant(_))
            ),
            "a second concurrent attempt is refused"
        );
        db.write(|c| abandon(c, &i.id, prev)).unwrap();
        assert_eq!(
            db.read(|c| get(c, &i.id)).unwrap().status,
            InstallStatus::Broken
        );
        assert_eq!(db.read(list).unwrap().len(), 1, "never duplicated");
    }

    #[test]
    fn broken_install_loses_default() {
        let db = db();
        let id = db
            .write(|c| {
                let i = installed(c, "php", "8.4.1", "p")?;
                set_global_default(c, &i.id)?;
                set_status(c, &i.id, InstallStatus::Broken)?;
                Ok(i.id)
            })
            .unwrap();
        assert!(!db.read(|c| get(c, &id)).unwrap().is_global_default);
    }

    #[test]
    fn failed_transaction_leaves_previous_default() {
        let db = db();
        let (a, b) = db
            .write(|c| {
                let a = installed(c, "php", "8.3.0", "a")?;
                let b = installed(c, "php", "8.4.1", "b")?;
                set_global_default(c, &a.id)?;
                Ok((a.id, b.id))
            })
            .unwrap();
        let res: DbResult<()> = db.write(|c| {
            set_global_default(c, &b)?;
            Err(DbError::Invariant(
                "verification of new default failed".into(),
            ))
        });
        assert!(res.is_err());
        assert!(db.read(|c| get(c, &a)).unwrap().is_global_default);
        assert!(!db.read(|c| get(c, &b)).unwrap().is_global_default);
    }
}
