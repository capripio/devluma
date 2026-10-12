//! Startup reconciliation (trd.md §14, schema.md §5). Runs once before the UI loads data:
//!
//! - `queued`/`running` operations were interrupted → `needs_recovery` (never deleted).
//! - `installing`/`uninstalling` installations were interrupted → `broken`.
//! - `installed` installations whose files are gone → `broken`.
//! - Every service's cached state → `unknown` until it is probed again.
//!
//! Nothing here deletes files, starts services or retries work automatically.

use std::path::Path;

use serde::Serialize;

use crate::domain::operation::OperationStatus;
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::{self as ops, EventLevel};
use crate::persistence::{Database, DbResult, services};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReport {
    pub operations_needing_recovery: usize,
    pub installations_marked_broken: usize,
    pub services_marked_unknown: usize,
}

pub fn reconcile(db: &Database) -> DbResult<RecoveryReport> {
    db.write(|c| {
        let mut report = RecoveryReport::default();

        for op in ops::list_unfinished(c)? {
            if op.status == OperationStatus::NeedsRecovery {
                continue;
            }
            ops::transition(c, &op.id, OperationStatus::NeedsRecovery, None)?;
            ops::append_event(
                c,
                &op.id,
                "recovery",
                EventLevel::Warn,
                "Devluma closed before this operation finished. Review it before retrying.",
                &serde_json::json!({ "previousStatus": op.status }),
            )?;
            report.operations_needing_recovery += 1;
        }

        for inst in installations::list(c)? {
            let broken = match inst.status {
                InstallStatus::Installing | InstallStatus::Uninstalling => true,
                InstallStatus::Installed => !Path::new(&inst.install_path).exists(),
                InstallStatus::Broken | InstallStatus::Removed => false,
            };
            if broken {
                installations::set_status(c, &inst.id, InstallStatus::Broken)?;
                report.installations_marked_broken += 1;
            }
        }

        report.services_marked_unknown = services::mark_all_unknown(c)?;
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::installations::fixtures::installed;
    use crate::persistence::operations::NewOperation;
    use crate::persistence::packages::fixtures::seed;
    use crate::persistence::test_support::db;

    #[test]
    fn interrupted_work_is_flagged_not_lost() {
        let db = db();
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("php-8.3").to_string_lossy().into_owned();
        std::fs::create_dir(&present).unwrap();

        let (running, done, ok_inst, missing_inst, half_inst) = db
            .write(|c| {
                let new = NewOperation {
                    operation_type: "install_package",
                    ..Default::default()
                };
                let running = ops::create(c, &new)?;
                ops::transition(c, &running.id, OperationStatus::Running, None)?;
                let done = ops::create(c, &new)?;
                ops::transition(c, &done.id, OperationStatus::Running, None)?;
                ops::transition(c, &done.id, OperationStatus::Succeeded, None)?;

                let ok = installed(c, "php", "8.3.0", &present)?;
                let missing = installed(c, "php", "8.4.1", "Z:\\definitely\\not\\here")?;
                seed(c, "node", &["22.0.0"])?;
                let half = installations::begin(c, "node-22.0.0", "x")?;
                Ok((running.id, done.id, ok.id, missing.id, half.id))
            })
            .unwrap();

        let report = reconcile(&db).unwrap();
        assert_eq!(
            report,
            RecoveryReport {
                operations_needing_recovery: 1,
                installations_marked_broken: 2,
                services_marked_unknown: 0,
            }
        );

        db.read(|c| {
            assert_eq!(
                ops::get(c, &running)?.status,
                OperationStatus::NeedsRecovery
            );
            assert_eq!(ops::get(c, &done)?.status, OperationStatus::Succeeded);
            assert_eq!(
                installations::get(c, &ok_inst)?.status,
                InstallStatus::Installed
            );
            assert_eq!(
                installations::get(c, &missing_inst)?.status,
                InstallStatus::Broken
            );
            assert_eq!(
                installations::get(c, &half_inst)?.status,
                InstallStatus::Broken
            );
            let events = ops::list_events(c, &running)?;
            assert_eq!(events.last().unwrap().event_type, "recovery");
            Ok(())
        })
        .unwrap();

        // Idempotent: a second startup changes nothing more.
        assert_eq!(
            reconcile(&db).unwrap(),
            RecoveryReport {
                operations_needing_recovery: 0,
                installations_marked_broken: 0,
                services_marked_unknown: 0,
            }
        );
    }

    /// Cross-cutting: an app upgrade finds a database from an older Devluma (schema v1) with
    /// work that was running when it closed. The upgrade backs the file up, migrates, keeps
    /// the data, and the interrupted work is flagged for review — never silently dropped.
    #[test]
    fn upgrade_from_an_older_schema_with_pending_work() {
        use crate::persistence::migrations::MIGRATIONS;
        use crate::persistence::operations::NewOperation;
        let dir = tempfile::tempdir().unwrap();
        let (db_path, backups) = (dir.path().join("app.db"), dir.path().join("backups"));
        let op_id = {
            let old = Database::open_with_migrations(&db_path, &backups, &MIGRATIONS[..1]).unwrap();
            old.write(|c| {
                let op = ops::create(
                    c,
                    &NewOperation {
                        operation_type: "install_package",
                        target_type: None,
                        target_id: Some("php@8.4.26@x64"),
                    },
                )?;
                ops::transition(c, &op.id, OperationStatus::Running, None)?;
                Ok(op.id)
            })
            .unwrap()
        };

        let db = Database::open_with_migrations(&db_path, &backups, MIGRATIONS).unwrap();
        assert!(
            db.migration_backup().is_some_and(|p| p.is_file()),
            "backup before upgrading"
        );
        let latest = MIGRATIONS.iter().map(|m| m.version).max().unwrap();
        let version: i64 = db
            .read(|c| {
                Ok(
                    c.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(version, latest);

        let report = reconcile(&db).unwrap();
        assert_eq!(report.operations_needing_recovery, 1);
        let op = db.read(|c| ops::get(c, &op_id)).unwrap();
        assert_eq!(op.status, OperationStatus::NeedsRecovery);
        assert_eq!(
            op.target_id.as_deref(),
            Some("php@8.4.26@x64"),
            "data kept across the upgrade"
        );
        // Tables added by later migrations exist and are usable.
        db.read(|c| {
            Ok(
                c.query_row("SELECT COUNT(*) FROM database_backups", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    }
}
