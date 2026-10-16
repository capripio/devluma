//! Hosts-file workflow (app_flow.md §10): read → plan (exact diff) → back up → write
//! (directly if the user may, otherwise through the elevated helper) → re-read and verify.
//! Only Devluma's own tagged lines (`# managed by devluma`, ADR 0016) are ever changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::domain::hosts::{self, HostsFile, HostsLine, ManagedEntry};
use crate::error::ErrorCode;
use crate::persistence::Database;
use crate::persistence::hosts::{self as repo, HostsBackup, HostsChange, NewBackup};
use crate::platform::windows::elevate::{self, ElevateError};
use crate::platform::windows::hosts_file::{
    self, HostsRequest, HostsResult, REQUEST_FILE, RESULT_FILE, VERB_APPLY, sha256_hex,
};
use crate::services::supervisor::behavior::{Failure, failure};
use crate::state::DataPaths;

/// How a planned change reaches the file.
pub trait HostsWriter: Send + Sync {
    /// True if writing will show a Windows administrator prompt.
    fn needs_elevation(&self) -> bool;
    fn write(&self, req: &HostsRequest, op_dir: &Path) -> Result<HostsResult, Failure>;
}

/// Writes in-process (the user can already write the file, or tests use a temp file).
pub struct DirectWriter {
    pub hosts_path: PathBuf,
    pub backups_dir: PathBuf,
}

impl HostsWriter for DirectWriter {
    fn needs_elevation(&self) -> bool {
        false
    }
    fn write(&self, req: &HostsRequest, _op_dir: &Path) -> Result<HostsResult, Failure> {
        Ok(hosts_file::apply(&self.hosts_path, &self.backups_dir, req))
    }
}

/// Hands the request to `devluma-elevate.exe` (one UAC prompt).
pub struct ElevatedWriter;

impl HostsWriter for ElevatedWriter {
    fn needs_elevation(&self) -> bool {
        true
    }
    fn write(&self, req: &HostsRequest, op_dir: &Path) -> Result<HostsResult, Failure> {
        std::fs::create_dir_all(op_dir).map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let body =
            serde_json::to_vec(req).map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let request = op_dir.join(REQUEST_FILE);
        let _ = std::fs::remove_file(op_dir.join(RESULT_FILE));
        std::fs::write(&request, &body).map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let helper = elevate::helper_path().ok_or_else(|| {
            failure(
                ErrorCode::Internal,
                "Could not locate the elevation helper.",
            )
        })?;
        let code = elevate::run_elevated(
            &helper,
            &[request.display().to_string(), sha256_hex(&body)],
            Duration::from_secs(120),
        )
        .map_err(|e| match e {
            ElevateError::Declined => failure(
                ErrorCode::PermissionDenied,
                "Administrator approval was declined, so the hosts file was not changed.",
            ),
            other => failure(
                ErrorCode::PermissionDenied,
                format!("Could not update the hosts file: {other}."),
            ),
        })?;
        let result: HostsResult = std::fs::read(op_dir.join(RESULT_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| {
                failure(
                    ErrorCode::Internal,
                    format!(
                        "The elevation helper exited (code {code}) without reporting a result."
                    ),
                )
            })?;
        Ok(result)
    }
}

/// Production writer: direct when the hosts file is writable without elevation.
pub fn default_writer(paths: &DataPaths) -> (PathBuf, Arc<dyn HostsWriter>) {
    let hosts_path = hosts_file::system_hosts_path()
        .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts"));
    let writable = std::fs::OpenOptions::new()
        .append(true)
        .open(&hosts_path)
        .is_ok();
    let writer: Arc<dyn HostsWriter> = if writable {
        Arc::new(DirectWriter {
            hosts_path: hosts_path.clone(),
            backups_dir: paths.backups.join("hosts"),
        })
    } else {
        Arc::new(ElevatedWriter)
    };
    (hosts_path, writer)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsView {
    pub path: String,
    pub needs_elevation: bool,
    /// Devluma's own lines only (tagged `# managed by devluma`, loopback).
    pub entries: Vec<HostsLine>,
    /// How many other entries the file has; Devluma never lists or edits them.
    pub other_count: usize,
    /// Lines carrying Devluma's tag on a non-loopback address: left alone, shown as a warning.
    pub stray_tags: Vec<HostsLine>,
    pub backups: Vec<HostsBackup>,
}

/// A previewed change. `sha256_before` pins the exact file the user saw.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsPlan {
    pub path: String,
    pub sha256_before: String,
    pub entries: Vec<ManagedEntry>,
    pub changes: Vec<HostsChange>,
    pub diff: String,
    pub needs_elevation: bool,
    pub unchanged: bool,
}

pub struct HostsService {
    db: Arc<Database>,
    paths: DataPaths,
    hosts_path: PathBuf,
    writer: Arc<dyn HostsWriter>,
}

impl HostsService {
    pub fn new(
        db: Arc<Database>,
        paths: DataPaths,
        hosts_path: PathBuf,
        writer: Arc<dyn HostsWriter>,
    ) -> Self {
        Self {
            db,
            paths,
            hosts_path,
            writer,
        }
    }

    fn read_file(&self) -> Result<(String, String), Failure> {
        let bytes = std::fs::read(&self.hosts_path).map_err(|e| {
            failure(
                ErrorCode::PermissionDenied,
                format!("Could not read the hosts file: {e}"),
            )
        })?;
        let sha = sha256_hex(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| {
            failure(
                ErrorCode::ValidationError,
                "The hosts file isn't UTF-8 text; Devluma won't edit it.",
            )
        })?;
        Ok((text, sha))
    }

    pub fn view(&self) -> Result<HostsView, Failure> {
        let (text, _) = self.read_file()?;
        let backups = self.db.read(|c| repo::list(c, 20)).unwrap_or_default();
        let file = HostsFile::parse(&text);
        Ok(HostsView {
            path: self.hosts_path.display().to_string(),
            needs_elevation: self.writer.needs_elevation(),
            other_count: file.other_count(),
            stray_tags: file.stray_tags.clone(),
            entries: file.entries.into_iter().filter(|e| e.managed).collect(),
            backups,
        })
    }

    pub fn managed(&self) -> Result<Vec<ManagedEntry>, Failure> {
        let (text, _) = self.read_file()?;
        Ok(HostsFile::parse(&text).managed())
    }

    /// Plans replacing Devluma's entries with `desired`.
    pub fn plan(&self, desired: &[ManagedEntry]) -> Result<HostsPlan, Failure> {
        let (text, sha) = self.read_file()?;
        let file = HostsFile::parse(&text);
        let entries = hosts::validate(desired)
            .map_err(|e| failure(ErrorCode::ValidationError, e.to_string()))?;
        let new_text = file
            .with_managed(&entries)
            .map_err(|e| failure(ErrorCode::ValidationError, e.to_string()))?;

        let before: BTreeMap<String, String> = file
            .managed()
            .into_iter()
            .map(|e| (e.domain, e.ip))
            .collect();
        let after: BTreeMap<String, String> = entries
            .iter()
            .map(|e| (e.domain.clone(), e.ip.clone()))
            .collect();
        let mut changes = Vec::new();
        for (d, ip) in &after {
            match before.get(d) {
                None => changes.push(change(d, ip, "add")),
                Some(old) if old != ip => changes.push(change(d, ip, "update")),
                _ => {}
            }
        }
        for (d, ip) in &before {
            if !after.contains_key(d) {
                changes.push(change(d, ip, "remove"));
            }
        }
        Ok(HostsPlan {
            path: self.hosts_path.display().to_string(),
            sha256_before: sha,
            unchanged: new_text == text,
            diff: hosts::diff(&text, &new_text, &self.hosts_path.display().to_string()),
            entries,
            changes,
            needs_elevation: self.writer.needs_elevation(),
        })
    }

    /// Plans adding/updating one domain and removing others, keeping the rest.
    pub fn plan_change(
        &self,
        add: &[ManagedEntry],
        remove: &[String],
    ) -> Result<HostsPlan, Failure> {
        let mut desired: Vec<ManagedEntry> = self
            .managed()?
            .into_iter()
            .filter(|e| !remove.contains(&e.domain) && !add.iter().any(|a| a.domain == e.domain))
            .collect();
        desired.extend(add.iter().cloned());
        self.plan(&desired)
    }

    /// Writes a previously shown plan (fails if the file changed since), records the backup
    /// and verifies the result by re-reading the file.
    pub fn apply(
        &self,
        plan: &HostsPlan,
        reason: &str,
        operation_id: &str,
    ) -> Result<HostsResult, Failure> {
        if plan.unchanged {
            return Ok(HostsResult {
                ok: true,
                error_code: None,
                message: "The hosts file already has these entries.".into(),
                sha256_before: Some(plan.sha256_before.clone()),
                sha256_after: Some(plan.sha256_before.clone()),
                backup_path: None,
                changed: false,
            });
        }
        let req = HostsRequest {
            verb: VERB_APPLY.into(),
            expected_sha256_before: plan.sha256_before.clone(),
            entries: plan.entries.clone(),
        };
        let result = self
            .writer
            .write(&req, &self.paths.staging.join(operation_id))?;
        if !result.ok {
            let code = match result.error_code.as_deref() {
                Some("PERMISSION_DENIED") => ErrorCode::PermissionDenied,
                Some("VALIDATION_ERROR") => ErrorCode::ValidationError,
                _ => ErrorCode::Internal,
            };
            return Err(failure(code, result.message));
        }
        // Independent verification from this (unelevated) process.
        let (_, sha_now) = self.read_file()?;
        if Some(&sha_now) != result.sha256_after.as_ref() {
            return Err(failure(
                ErrorCode::Internal,
                "The hosts file changed again right after writing; check it on the Hosts File page.",
            ));
        }
        let mut result = result;
        if let (true, Some(backup)) = (result.changed, result.backup_path.clone()) {
            let backup = &backup;
            let recorded = self.db.write(|c| {
                repo::record(
                    c,
                    &NewBackup {
                        original_path: &plan.path,
                        backup_path: backup,
                        sha256_before: &plan.sha256_before,
                        sha256_after: &sha_now,
                        reason,
                        operation_id: Some(operation_id),
                    },
                    &plan.changes,
                )
            });
            // The file is already written and verified; don't fail, but don't hide it either.
            if recorded.is_err() {
                result.message = format!(
                    "{} The backup ({backup}) couldn't be recorded in Devluma's history, so it won't appear in the restore list.",
                    result.message
                );
            }
        }
        Ok(result)
    }

    /// Devluma's entries as they were in a backup — used to put them back.
    pub fn entries_in_backup(&self, backup_id: &str) -> Result<Vec<ManagedEntry>, Failure> {
        let b = self
            .db
            .read(|c| repo::get(c, backup_id))
            .map_err(|e| failure(ErrorCode::NotFound, e.to_string()))?;
        let text = std::fs::read_to_string(&b.backup_path).map_err(|e| {
            failure(
                ErrorCode::NotFound,
                format!("The backup file is missing: {e}"),
            )
        })?;
        // Backups from before tagged lines (the old marked block) hold no tagged entries.
        Ok(HostsFile::parse(&text).managed())
    }
}

fn change(domain: &str, ip: &str, action: &str) -> HostsChange {
    HostsChange {
        domain: domain.into(),
        ip_address: ip.into(),
        action: action.into(),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A service over a temp hosts file written directly (no elevation).
    pub fn service(db: Arc<Database>, paths: &DataPaths, initial: &str) -> (HostsService, PathBuf) {
        let hosts = paths.root.join("hosts-under-test");
        std::fs::write(&hosts, initial).unwrap();
        let writer = Arc::new(DirectWriter {
            hosts_path: hosts.clone(),
            backups_dir: paths.backups.join("hosts"),
        });
        (
            HostsService::new(db, paths.clone(), hosts.clone(), writer),
            hosts,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::service;
    use super::*;
    use crate::persistence::test_support::db;

    fn e(d: &str) -> ManagedEntry {
        ManagedEntry {
            ip: "127.0.0.1".into(),
            domain: d.into(),
        }
    }

    fn setup(initial: &str) -> (tempfile::TempDir, HostsService, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let database = Arc::new(db());
        // Backups reference real journal operations (foreign key).
        database
            .write(|c| {
                for id in ["op1", "op2", "op3", "op"] {
                    c.execute(
                        "INSERT INTO operations (id, operation_type, status, started_at) VALUES (?1, 'apply_hosts', 'running', 't')",
                        [id],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let (svc, hosts) = service(database, &paths, initial);
        (dir, svc, hosts)
    }

    #[test]
    fn plan_apply_restore_round_trip() {
        let (_d, svc, hosts) = setup("127.0.0.1 localhost\r\n");
        let plan = svc.plan_change(&[e("a.test")], &[]).unwrap();
        assert_eq!(plan.changes, vec![change("a.test", "127.0.0.1", "add")]);
        assert!(
            plan.diff
                .contains("+127.0.0.1\ta.test\t# managed by devluma")
        );
        let first = svc.apply(&plan, "Site a.test", "op1").unwrap();
        assert!(first.changed);

        let plan = svc.plan_change(&[e("b.test")], &["a.test".into()]).unwrap();
        assert_eq!(plan.changes.len(), 2);
        svc.apply(&plan, "Site b.test", "op2").unwrap();
        assert_eq!(svc.managed().unwrap(), vec![e("b.test")]);

        // Restore Devluma's entries from the first backup (taken before a.test was added).
        let view = svc.view().unwrap();
        assert_eq!(view.backups.len(), 2);
        let oldest = view.backups.last().unwrap();
        assert_eq!(oldest.changes[0].domain, "a.test");
        let entries = svc.entries_in_backup(&oldest.id).unwrap();
        let plan = svc.plan(&entries).unwrap();
        svc.apply(&plan, "Restore", "op3").unwrap();
        assert_eq!(
            std::fs::read_to_string(&hosts).unwrap(),
            "127.0.0.1 localhost\r\n"
        );
    }

    #[test]
    fn stale_plans_are_rejected() {
        let (_d, svc, hosts) = setup("127.0.0.1 localhost\n");
        let plan = svc.plan_change(&[e("a.test")], &[]).unwrap();
        std::fs::write(&hosts, "127.0.0.1 localhost\n10.0.0.1 other\n").unwrap();
        let err = svc.apply(&plan, "x", "op").unwrap_err();
        assert_eq!(err.code, ErrorCode::ValidationError);
        assert!(err.message.contains("changed after you reviewed"));
        assert!(!std::fs::read_to_string(&hosts).unwrap().contains("a.test"));
    }

    #[test]
    fn unchanged_plans_write_nothing() {
        let (_d, svc, _hosts) = setup("127.0.0.1 localhost\n");
        let plan = svc.plan(&[]).unwrap();
        assert!(plan.unchanged && plan.changes.is_empty());
        assert!(!svc.apply(&plan, "x", "op").unwrap().changed);
        assert!(svc.view().unwrap().backups.is_empty());
    }

    #[test]
    fn view_lists_only_devluma_lines_and_counts_the_rest() {
        let (_d, svc, _hosts) = setup(
            "127.0.0.1 localhost\n127.0.0.1 child.local #laragon magic!\n127.0.0.1\tshop.test\t# managed by devluma\n10.0.0.7 nas.test # managed by devluma\n",
        );
        let v = svc.view().unwrap();
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].hostnames, vec!["shop.test"]);
        assert_eq!(v.other_count, 3);
        assert_eq!(v.stray_tags.len(), 1);
        assert_eq!(v.stray_tags[0].line, 4);
    }
}
