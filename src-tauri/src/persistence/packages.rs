//! Catalog identity (`packages`) and versions (`package_versions`). Catalog rows are
//! written from validated manifests only (Phase 3); manifests never carry commands.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::{DbError, DbResult, now};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub id: String,
    pub display_name: String,
    pub category: String,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub homepage_url: Option<String>,
    pub license_id: Option<String>,
    pub license_url: Option<String>,
    pub catalog_source: String,
    pub catalog_version: String,
    pub support_policy_url: Option<String>,
}

/// Inserts or updates catalog identity; `created_at` is preserved on update.
pub fn upsert(conn: &Connection, p: &Package) -> DbResult<()> {
    let ts = now();
    conn.execute(
        "INSERT INTO packages (id, display_name, category, description, publisher, homepage_url,
                               license_id, license_url, catalog_source, catalog_version,
                               support_policy_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
         ON CONFLICT (id) DO UPDATE SET
           display_name = excluded.display_name, category = excluded.category,
           description = excluded.description, publisher = excluded.publisher,
           homepage_url = excluded.homepage_url, license_id = excluded.license_id,
           license_url = excluded.license_url, catalog_source = excluded.catalog_source,
           catalog_version = excluded.catalog_version,
           support_policy_url = excluded.support_policy_url, updated_at = excluded.updated_at",
        params![
            p.id,
            p.display_name,
            p.category,
            p.description,
            p.publisher,
            p.homepage_url,
            p.license_id,
            p.license_url,
            p.catalog_source,
            p.catalog_version,
            p.support_policy_url,
            ts
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Package> {
    conn.query_row(
        "SELECT id, display_name, category, description, publisher, homepage_url, license_id,
                license_url, catalog_source, catalog_version, support_policy_url
         FROM packages WHERE id = ?1",
        [id],
        |r| {
            Ok(Package {
                id: r.get(0)?,
                display_name: r.get(1)?,
                category: r.get(2)?,
                description: r.get(3)?,
                publisher: r.get(4)?,
                homepage_url: r.get(5)?,
                license_id: r.get(6)?,
                license_url: r.get(7)?,
                catalog_source: r.get(8)?,
                catalog_version: r.get(9)?,
                support_policy_url: r.get(10)?,
            })
        },
    )
    .optional()?
    .ok_or(DbError::NotFound { entity: "package" })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageVersion {
    pub id: String,
    pub package_id: String,
    pub version: String,
    pub architecture: String,
    pub os_constraints: serde_json::Value,
    pub source_url: String,
    pub publisher_signature_url: Option<String>,
    pub sha256: Option<String>,
    pub integrity_policy: String,
    pub archive_type: String,
    pub expected_size_bytes: Option<i64>,
    pub release_date: Option<String>,
    pub end_of_life_date: Option<String>,
    pub manifest: serde_json::Value,
}

pub fn insert_version(conn: &Connection, v: &PackageVersion) -> DbResult<()> {
    conn.execute(
        "INSERT INTO package_versions (id, package_id, version, architecture, os_constraints_json,
                                       source_url, publisher_signature_url, sha256, integrity_policy,
                                       archive_type, expected_size_bytes, release_date,
                                       end_of_life_date, manifest_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            v.id,
            v.package_id,
            v.version,
            v.architecture,
            serde_json::to_string(&v.os_constraints)?,
            v.source_url,
            v.publisher_signature_url,
            v.sha256,
            v.integrity_policy,
            v.archive_type,
            v.expected_size_bytes,
            v.release_date,
            v.end_of_life_date,
            serde_json::to_string(&v.manifest)?,
            now()
        ],
    )?;
    Ok(())
}

/// Inserts or refreshes a version row from the current catalog (same deterministic id).
pub fn upsert_version(conn: &Connection, v: &PackageVersion) -> DbResult<()> {
    conn.execute(
        "INSERT INTO package_versions (id, package_id, version, architecture, os_constraints_json,
                                       source_url, publisher_signature_url, sha256, integrity_policy,
                                       archive_type, expected_size_bytes, release_date,
                                       end_of_life_date, manifest_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (id) DO UPDATE SET
           os_constraints_json = excluded.os_constraints_json, source_url = excluded.source_url,
           publisher_signature_url = excluded.publisher_signature_url, sha256 = excluded.sha256,
           integrity_policy = excluded.integrity_policy, archive_type = excluded.archive_type,
           expected_size_bytes = excluded.expected_size_bytes, release_date = excluded.release_date,
           end_of_life_date = excluded.end_of_life_date, manifest_json = excluded.manifest_json",
        params![
            v.id,
            v.package_id,
            v.version,
            v.architecture,
            serde_json::to_string(&v.os_constraints)?,
            v.source_url,
            v.publisher_signature_url,
            v.sha256,
            v.integrity_policy,
            v.archive_type,
            v.expected_size_bytes,
            v.release_date,
            v.end_of_life_date,
            serde_json::to_string(&v.manifest)?,
            now()
        ],
    )?;
    Ok(())
}

pub fn get_version(conn: &Connection, id: &str) -> DbResult<PackageVersion> {
    conn.query_row(
        "SELECT id, package_id, version, architecture, os_constraints_json, source_url,
                publisher_signature_url, sha256, integrity_policy, archive_type,
                expected_size_bytes, release_date, end_of_life_date, manifest_json
         FROM package_versions WHERE id = ?1",
        [id],
        |r| {
            let os: String = r.get(4)?;
            let manifest: String = r.get(13)?;
            Ok(PackageVersion {
                id: r.get(0)?,
                package_id: r.get(1)?,
                version: r.get(2)?,
                architecture: r.get(3)?,
                os_constraints: serde_json::from_str(&os).unwrap_or_default(),
                source_url: r.get(5)?,
                publisher_signature_url: r.get(6)?,
                sha256: r.get(7)?,
                integrity_policy: r.get(8)?,
                archive_type: r.get(9)?,
                expected_size_bytes: r.get(10)?,
                release_date: r.get(11)?,
                end_of_life_date: r.get(12)?,
                manifest: serde_json::from_str(&manifest).unwrap_or_default(),
            })
        },
    )
    .optional()?
    .ok_or(DbError::NotFound {
        entity: "package version",
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn package(id: &str) -> Package {
        Package {
            id: id.into(),
            display_name: id.to_uppercase(),
            category: "runtime".into(),
            description: None,
            publisher: Some("Fixture".into()),
            homepage_url: None,
            license_id: Some("MIT".into()),
            license_url: None,
            catalog_source: "fixture".into(),
            catalog_version: "1".into(),
            support_policy_url: None,
        }
    }

    pub fn version(id: &str, package_id: &str, version: &str) -> PackageVersion {
        PackageVersion {
            id: id.into(),
            package_id: package_id.into(),
            version: version.into(),
            architecture: "x64".into(),
            os_constraints: serde_json::json!({ "minBuild": 19045 }),
            source_url: format!("https://example.invalid/{package_id}-{version}.zip"),
            publisher_signature_url: None,
            sha256: Some("0".repeat(64)),
            integrity_policy: "sha256-required".into(),
            archive_type: "zip".into(),
            expected_size_bytes: Some(1024),
            release_date: None,
            end_of_life_date: None,
            manifest: serde_json::json!({}),
        }
    }

    /// Inserts `pkg` with one version per entry of `versions` (ids `<pkg>-<ver>`).
    pub fn seed(conn: &Connection, pkg: &str, versions: &[&str]) -> DbResult<()> {
        upsert(conn, &package(pkg))?;
        for v in versions {
            insert_version(conn, &version(&format!("{pkg}-{v}"), pkg, v))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::persistence::{is_constraint_violation, test_support::db};

    #[test]
    fn upsert_preserves_identity_and_updates_fields() {
        let db = db();
        db.write(|c| upsert(c, &package("php"))).unwrap();
        let mut p = package("php");
        p.display_name = "PHP".into();
        db.write(|c| upsert(c, &p)).unwrap();
        assert_eq!(db.read(|c| get(c, "php")).unwrap().display_name, "PHP");
    }

    #[test]
    fn versions_are_unique_per_package_version_architecture() {
        let db = db();
        db.write(|c| seed(c, "php", &["8.4.1"])).unwrap();
        let err = db
            .write(|c| insert_version(c, &version("dup", "php", "8.4.1")))
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlite(ref e) if is_constraint_violation(e)));
    }

    #[test]
    fn version_requires_existing_package_and_valid_architecture() {
        let db = db();
        let err = db
            .write(|c| insert_version(c, &version("v", "missing", "1.0")))
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlite(ref e) if is_constraint_violation(e)));

        db.write(|c| upsert(c, &package("node"))).unwrap();
        let mut v = version("v", "node", "22.0.0");
        v.architecture = "sparc".into();
        assert!(db.write(|c| insert_version(c, &v)).is_err());
    }
}
