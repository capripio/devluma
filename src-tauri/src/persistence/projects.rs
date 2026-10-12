//! Projects and project-scoped runtime pins. Deleting a project removes metadata and links
//! only — never project files, package installations or database data (schema.md §5).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::installations::{self, InstallStatus};
use super::{DbError, DbResult, new_id, now};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub project_type: Option<String>,
    pub local_domain: Option<String>,
    pub notes: Option<String>,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_opened_at: Option<String>,
}

const COLUMNS: &str = "id, name, root_path, project_type, local_domain, notes, is_favorite, created_at, \
                       updated_at, last_opened_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        root_path: r.get(2)?,
        project_type: r.get(3)?,
        local_domain: r.get(4)?,
        notes: r.get(5)?,
        is_favorite: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        last_opened_at: r.get(9)?,
    })
}

#[derive(Debug, Clone, Default)]
pub struct ProjectFields {
    pub name: String,
    pub root_path: String,
    pub project_type: Option<String>,
    pub local_domain: Option<String>,
    pub notes: Option<String>,
    pub is_favorite: bool,
}

fn validate(f: &ProjectFields) -> DbResult<()> {
    if f.name.trim().is_empty() {
        return Err(DbError::Invariant("Project name is required.".into()));
    }
    if f.root_path.trim().is_empty() {
        return Err(DbError::Invariant("Project folder is required.".into()));
    }
    Ok(())
}

pub fn create(conn: &Connection, f: &ProjectFields) -> DbResult<Project> {
    validate(f)?;
    let id = new_id();
    let ts = now();
    conn.execute(
        "INSERT INTO projects (id, name, root_path, project_type, local_domain, notes, is_favorite, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            id,
            f.name.trim(),
            f.root_path,
            f.project_type,
            f.local_domain,
            f.notes,
            f.is_favorite,
            ts
        ],
    )?;
    get(conn, &id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Project> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM projects WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or(DbError::NotFound { entity: "project" })
}

pub fn list(conn: &Connection) -> DbResult<Vec<Project>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM projects ORDER BY is_favorite DESC, updated_at DESC"
    ))?;
    Ok(stmt.query_map([], from_row)?.collect::<Result<_, _>>()?)
}

pub fn update(conn: &Connection, id: &str, f: &ProjectFields) -> DbResult<Project> {
    validate(f)?;
    let n = conn.execute(
        "UPDATE projects SET name = ?2, root_path = ?3, project_type = ?4, local_domain = ?5,
                             notes = ?6, is_favorite = ?7, updated_at = ?8
         WHERE id = ?1",
        params![
            id,
            f.name.trim(),
            f.root_path,
            f.project_type,
            f.local_domain,
            f.notes,
            f.is_favorite,
            now()
        ],
    )?;
    if n == 0 {
        return Err(DbError::NotFound { entity: "project" });
    }
    get(conn, id)
}

/// Removes project metadata (and, via cascade, its pins, sites and service links).
pub fn remove(conn: &Connection, id: &str) -> DbResult<()> {
    if conn.execute("DELETE FROM projects WHERE id = ?1", [id])? == 0 {
        return Err(DbError::NotFound { entity: "project" });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePin {
    pub project_id: String,
    pub package_id: String,
    pub package_installation_id: String,
}

/// Pins a project to a verified installation, replacing any previous pin for that package.
/// Returns the previous installation id (for rollback). Never touches the global default.
pub fn pin_runtime(
    conn: &Connection,
    project_id: &str,
    installation_id: &str,
) -> DbResult<Option<String>> {
    get(conn, project_id)?;
    let inst = installations::get(conn, installation_id)?;
    if inst.status != InstallStatus::Installed || inst.verified_at.is_none() {
        return Err(DbError::Invariant(
            "Only a verified, installed version can be pinned.".into(),
        ));
    }
    let previous: Option<String> = conn
        .query_row(
            "SELECT package_installation_id FROM project_runtimes WHERE project_id = ?1 AND package_id = ?2",
            params![project_id, inst.package_id],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute(
        "INSERT INTO project_runtimes (id, project_id, package_id, package_installation_id, selection_scope, created_at)
         VALUES (?1, ?2, ?3, ?4, 'project', ?5)
         ON CONFLICT (project_id, package_id) DO UPDATE SET
           package_installation_id = excluded.package_installation_id, created_at = excluded.created_at",
        params![new_id(), project_id, inst.package_id, installation_id, now()],
    )?;
    Ok(previous)
}

pub fn list_pins(conn: &Connection, project_id: &str) -> DbResult<Vec<RuntimePin>> {
    let mut stmt = conn.prepare(
        "SELECT project_id, package_id, package_installation_id FROM project_runtimes
         WHERE project_id = ?1 ORDER BY package_id",
    )?;
    Ok(stmt
        .query_map([project_id], |r| {
            Ok(RuntimePin {
                project_id: r.get(0)?,
                package_id: r.get(1)?,
                package_installation_id: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

/// Removes a project's pin so it follows the global default again. Returns the previous
/// installation id, if there was a pin.
pub fn unpin_runtime(
    conn: &Connection,
    project_id: &str,
    package_id: &str,
) -> DbResult<Option<String>> {
    let previous: Option<String> = conn
        .query_row(
            "SELECT package_installation_id FROM project_runtimes WHERE project_id = ?1 AND package_id = ?2",
            params![project_id, package_id],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute(
        "DELETE FROM project_runtimes WHERE project_id = ?1 AND package_id = ?2",
        params![project_id, package_id],
    )?;
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::installations::fixtures::installed;
    use crate::persistence::is_constraint_violation;
    use crate::persistence::test_support::db;

    fn fields(name: &str) -> ProjectFields {
        ProjectFields {
            name: name.into(),
            root_path: format!("D:\\code\\{name}"),
            ..Default::default()
        }
    }

    #[test]
    fn crud_round_trip() {
        let db = db();
        let p = db.write(|c| create(c, &fields("shop"))).unwrap();
        let mut f = fields("shop");
        f.local_domain = Some("shop.test".into());
        f.is_favorite = true;
        let p2 = db.write(|c| update(c, &p.id, &f)).unwrap();
        assert_eq!(p2.local_domain.as_deref(), Some("shop.test"));
        assert!(p2.updated_at >= p.updated_at);
        assert_eq!(db.read(list).unwrap().len(), 1);
        db.write(|c| remove(c, &p.id)).unwrap();
        assert!(matches!(
            db.read(|c| get(c, &p.id)),
            Err(DbError::NotFound { .. })
        ));
    }

    #[test]
    fn rejects_blank_name() {
        let db = db();
        assert!(matches!(
            db.write(|c| create(c, &fields("  "))),
            Err(DbError::Invariant(_))
        ));
    }

    #[test]
    fn pins_are_per_project_and_leave_global_default_alone() {
        let db = db();
        db.write(|c| {
            let a = create(c, &fields("a"))?;
            let b = create(c, &fields("b"))?;
            let php83 = installed(c, "php", "8.3.0", "p83")?;
            let php84 = installed(c, "php", "8.4.1", "p84")?;
            installations::set_global_default(c, &php83.id)?;

            pin_runtime(c, &a.id, &php83.id)?;
            pin_runtime(c, &b.id, &php83.id)?;
            let prev = pin_runtime(c, &a.id, &php84.id)?;

            assert_eq!(prev, Some(php83.id.clone()));
            assert_eq!(list_pins(c, &a.id)?[0].package_installation_id, php84.id);
            assert_eq!(list_pins(c, &b.id)?[0].package_installation_id, php83.id);
            assert!(installations::get(c, &php83.id)?.is_global_default);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn removing_a_project_cascades_pins_but_keeps_installations() {
        let db = db();
        let (p, inst) = db
            .write(|c| {
                let p = create(c, &fields("a"))?;
                let i = installed(c, "php", "8.4.1", "p")?;
                pin_runtime(c, &p.id, &i.id)?;
                remove(c, &p.id)?;
                Ok((p.id, i.id))
            })
            .unwrap();
        let pins: i64 = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM project_runtimes WHERE project_id = ?1",
                    [&p],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(pins, 0);
        assert!(db.read(|c| installations::get(c, &inst)).is_ok());
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = db();
        let err = db
            .write(|c| {
                Ok(c.execute(
                    "INSERT INTO project_runtimes (id, project_id, package_id, package_installation_id, selection_scope, created_at)
                     VALUES ('x', 'no-project', 'no-pkg', 'no-inst', 'project', 't')",
                    [],
                )?)
            })
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlite(ref e) if is_constraint_violation(e)));
    }
}
