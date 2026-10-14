//! Runtime selection (app_flow.md §5, ADR 0009): which installed version a project — or,
//! by default, everything — uses.
//!
//! Resolution order: project pin → global default → none. Switching is:
//! preview (exactly what changes) → verify the target executables actually run and report
//! the expected version → apply inside one transaction that re-resolves and checks the
//! result before committing (rollback = the transaction never commits). The previous
//! selection is journaled so it can be restored. Nothing outside Devluma's database changes:
//! no system PATH edits, no files in project folders.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::install::ExecutableProbe;
use super::journal::{Journal, ProgressSink};
use crate::domain::catalog::Catalog;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus, Installation};
use crate::persistence::operations::{EventLevel, NewOperation, Operation};
use crate::persistence::{Database, DbError, DbResult, projects};
use crate::security::paths;

pub const OPERATION_TYPE: &str = "switch_runtime";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Project,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRequest {
    pub package_id: String,
    pub scope: Scope,
    #[serde(default)]
    pub project_id: Option<String>,
    /// `None` with `scope: project` removes the pin (the project follows the global default).
    #[serde(default)]
    pub installation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    ProjectPin,
    GlobalDefault,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub installation_id: String,
    pub version: String,
    pub install_path: String,
    pub source: SelectionSource,
}

/// The installation a project (or, with `None`, a project without a pin) uses.
pub fn effective(
    c: &rusqlite::Connection,
    project_id: Option<&str>,
    package_id: &str,
) -> DbResult<Option<Selection>> {
    let all = installations::list(c)?;
    let version_of = |i: &Installation| {
        i.package_version_id
            .split('@')
            .nth(1)
            .unwrap_or(&i.package_version_id)
            .to_owned()
    };
    let to_selection = |i: &Installation, source| Selection {
        installation_id: i.id.clone(),
        version: version_of(i),
        install_path: i.install_path.clone(),
        source,
    };
    if let Some(pid) = project_id
        && let Some(pin) = projects::list_pins(c, pid)?
            .into_iter()
            .find(|p| p.package_id == package_id)
        && let Some(i) = all.iter().find(|i| i.id == pin.package_installation_id)
        && i.status == InstallStatus::Installed
    {
        return Ok(Some(to_selection(i, SelectionSource::ProjectPin)));
    }
    Ok(all
        .iter()
        .find(|i| {
            i.package_id == package_id
                && i.is_global_default
                && i.status == InstallStatus::Installed
        })
        .map(|i| to_selection(i, SelectionSource::GlobalDefault)))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchPreview {
    pub scope: Scope,
    pub package_id: String,
    pub package_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// What's in effect now for this scope (for a project: its pin or the inherited default).
    pub current: Option<Selection>,
    /// What will be in effect afterwards.
    pub target: Option<Selection>,
    /// Plain-language list of what changes.
    pub changes: Vec<String>,
    /// Plain-language list of what explicitly does *not* change.
    pub unchanged: Vec<String>,
    pub no_op: bool,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, msg, false)
}

pub fn preview(
    db: &Database,
    catalog: &Catalog,
    req: &SwitchRequest,
) -> Result<SwitchPreview, IpcError> {
    db.read(|c| Ok(build_preview(c, catalog, req)))?
}

fn build_preview(
    c: &rusqlite::Connection,
    catalog: &Catalog,
    req: &SwitchRequest,
) -> Result<SwitchPreview, IpcError> {
    let package_name = catalog
        .package(&req.package_id)
        .map(|p| p.manifest.display_name.clone())
        .unwrap_or_else(|| req.package_id.clone());

    let project = match (req.scope, &req.project_id) {
        (Scope::Project, Some(id)) => Some(projects::get(c, id)?),
        (Scope::Project, None) => return Err(invalid("Choose a project.")),
        (Scope::Global, _) => None,
    };

    let target_inst = match &req.installation_id {
        Some(id) => {
            let i = installations::get(c, id)?;
            if i.package_id != req.package_id {
                return Err(invalid("That installation belongs to a different package."));
            }
            if i.status != InstallStatus::Installed || i.verified_at.is_none() {
                return Err(invalid(
                    "Only a verified, installed version can be selected.",
                ));
            }
            Some(i)
        }
        None if req.scope == Scope::Global => {
            return Err(invalid("Choose a version to make the default."));
        }
        None => None,
    };

    let pid = project.as_ref().map(|p| p.id.as_str());
    let current = effective(c, pid, &req.package_id)?;
    let target = match &target_inst {
        Some(i) => Some(Selection {
            installation_id: i.id.clone(),
            version: i
                .package_version_id
                .split('@')
                .nth(1)
                .unwrap_or_default()
                .to_owned(),
            install_path: i.install_path.clone(),
            source: match req.scope {
                Scope::Global => SelectionSource::GlobalDefault,
                Scope::Project => SelectionSource::ProjectPin,
            },
        }),
        // Unpinning: the project falls back to the global default.
        None => effective(c, None, &req.package_id)?,
    };

    let mut changes = Vec::new();
    let mut unchanged = vec![
        "Your Windows PATH and environment variables are not modified.".to_owned(),
        "No files in your project folders are written.".to_owned(),
    ];
    let label = |s: &Option<Selection>| {
        s.as_ref().map_or("none".to_owned(), |s| {
            format!("{package_name} {}", s.version)
        })
    };
    let no_op = match (req.scope, &project) {
        (Scope::Global, _) => {
            let changing = current.as_ref().map(|s| &s.installation_id)
                != target.as_ref().map(|s| &s.installation_id);
            if changing {
                changes.push(format!(
                    "Default {package_name}: {} → {}.",
                    label(&current),
                    label(&target)
                ));
                changes.push(format!(
                    "Projects without their own {package_name} version will use {}.",
                    label(&target)
                ));
            }
            unchanged.push(format!(
                "Projects that pin their own {package_name} version keep it."
            ));
            !changing
        }
        (Scope::Project, Some(p)) => {
            let current_pin = projects::list_pins(c, &p.id)?
                .into_iter()
                .find(|x| x.package_id == req.package_id)
                .map(|x| x.package_installation_id);
            let changing = current_pin != req.installation_id;
            if changing {
                changes.push(match &target_inst {
                    Some(_) => format!("{}: {} → {}.", p.name, label(&current), label(&target)),
                    None => format!(
                        "{}: stop pinning {package_name}; use the default ({}).",
                        p.name,
                        label(&target)
                    ),
                });
            }
            unchanged.push(format!(
                "The default {package_name} and other projects are not affected."
            ));
            !changing
        }
        (Scope::Project, None) => unreachable!("validated above"),
    };

    Ok(SwitchPreview {
        scope: req.scope,
        package_id: req.package_id.clone(),
        package_name,
        project_name: project.map(|p| p.name),
        current,
        target,
        changes,
        unchanged,
        no_op,
    })
}

/// Verifies, applies and journals a switch. Blocking (runs probes); call off the UI thread.
pub fn apply(
    db: &Database,
    catalog: &Catalog,
    probe: &Arc<dyn ExecutableProbe>,
    sink: &dyn ProgressSink,
    req: &SwitchRequest,
) -> Result<Operation, IpcError> {
    let preview = preview(db, catalog, req)?;
    if preview.no_op {
        return Err(invalid(
            "Nothing to change: that's already the selected version.",
        ));
    }
    let target_key = match req.scope {
        Scope::Global => format!("global:{}", req.package_id),
        Scope::Project => format!(
            "project:{}:{}",
            req.project_id.as_deref().unwrap_or_default(),
            req.package_id
        ),
    };
    let journal = Journal::new(db, sink);
    let op = journal
        .start(&NewOperation {
            operation_type: OPERATION_TYPE,
            target_type: Some("runtime_selection"),
            target_id: Some(&target_key),
        })
        .map_err(IpcError::from)?;
    let finish = |result: Result<String, (ErrorCode, String)>| -> Result<Operation, IpcError> {
        match result {
            Ok(summary) => {
                let _ = journal.event(&op.id, EventLevel::Info, &summary, &serde_json::json!({}));
                journal.succeed(&op.id).map_err(IpcError::from)
            }
            Err((code, msg)) => journal.fail(&op.id, code, &msg).map_err(IpcError::from),
        }
    };
    journal.running(&op.id).map_err(IpcError::from)?;

    // 1. Verify the target really runs and is the version on record.
    if let Some(target) = &preview.target {
        journal
            .progress(&op.id, "Verifying executables", None, None)
            .map_err(IpcError::from)?;
        if let Err(msg) = verify_installation(db, catalog, probe, &req.package_id, target) {
            return finish(Err((ErrorCode::IntegrityError, msg)));
        }
    }

    // 2. Apply + re-resolve + check, all in one transaction.
    journal
        .progress(&op.id, "Applying", None, None)
        .map_err(IpcError::from)?;
    let expected = preview.target.as_ref().map(|t| t.installation_id.clone());
    let applied = db.write(|c| {
        let previous = match (req.scope, &req.installation_id) {
            (Scope::Global, Some(id)) => installations::set_global_default(c, id)?,
            (Scope::Project, Some(id)) => {
                projects::pin_runtime(c, req.project_id.as_deref().unwrap_or_default(), id)?
            }
            (Scope::Project, None) => projects::unpin_runtime(
                c,
                req.project_id.as_deref().unwrap_or_default(),
                &req.package_id,
            )?,
            (Scope::Global, None) => return Err(DbError::Invariant("Choose a version.".into())),
        };
        let pid =
            (req.scope == Scope::Project).then(|| req.project_id.as_deref().unwrap_or_default());
        let now_effective = effective(c, pid, &req.package_id)?.map(|s| s.installation_id);
        if now_effective != expected {
            // Returning an error rolls the transaction back: nothing changed.
            return Err(DbError::Invariant(
                "The new selection did not take effect; nothing was changed.".into(),
            ));
        }
        crate::persistence::operations::set_recovery_data(
            c,
            &op.id,
            &serde_json::json!({ "previousInstallationId": previous, "request": {
                "packageId": req.package_id, "scope": req.scope, "projectId": req.project_id,
                "installationId": req.installation_id,
            }}),
        )?;
        Ok(previous)
    });
    match applied {
        Ok(_) => finish(Ok(preview.changes.join(" "))),
        Err(e) => {
            let ipc = IpcError::from(e);
            finish(Err((ipc.code, ipc.message)))
        }
    }
}

/// Runs every executable the catalog lists for this version and checks the reported version.
pub fn verify_installation(
    db: &Database,
    catalog: &Catalog,
    probe: &Arc<dyn ExecutableProbe>,
    package_id: &str,
    target: &Selection,
) -> Result<(), String> {
    let pkg = catalog.package(package_id).ok_or_else(|| {
        "This package is no longer in the catalog, so it can't be verified.".to_owned()
    })?;
    let inst = db
        .read(|c| installations::get(c, &target.installation_id))
        .map_err(|e| e.to_string())?;
    let version = pkg
        .versions
        .iter()
        .find(|v| {
            crate::domain::catalog::version_row_id(package_id, &v.version, v.architecture)
                == inst.package_version_id
        })
        .ok_or_else(|| {
            "This version is no longer in the catalog, so it can't be verified.".to_owned()
        })?;
    let root = std::path::Path::new(&inst.install_path);
    for exe in &version.executables {
        let path = paths::join_relative(root, &exe.path)
            .map_err(|_| format!("Invalid executable path {}.", exe.path))?;
        if !path.is_file() {
            return Err(format!(
                "{} is missing from {}. Reinstall this version.",
                exe.path, inst.install_path
            ));
        }
        let output = super::install::probe_output(&**probe, pkg.adapter, &path)
            .map_err(|e| format!("Could not run {}: {e}", exe.path))?;
        match pkg.adapter.parse_version(&output) {
            Some(v) if v == exe.expected_version => {}
            Some(v) => {
                return Err(format!(
                    "{} reported {v}, expected {}.",
                    exe.path, exe.expected_version
                ));
            }
            None => {
                return Err(format!(
                    "{} did not report a recognizable version.",
                    exe.path
                ));
            }
        }
    }
    Ok(())
}

/// Directories to put first on PATH for a project's terminal (each runtime's effective
/// installation). Never applied system-wide.
/// True if `package_id` has no global default yet (first-run setup only fills gaps).
pub fn effective_default_missing(db: &Database, package_id: &str) -> bool {
    db.read(|c| effective(c, None, package_id))
        .map(|s| s.is_none())
        .unwrap_or(true)
}

pub fn project_path_prefix(
    db: &Database,
    catalog: &Catalog,
    project_id: &str,
) -> DbResult<Vec<String>> {
    path_prefix(db, catalog, Some(project_id))
}

/// Like `project_path_prefix`; `None` uses the global defaults (a terminal outside projects).
pub fn path_prefix(
    db: &Database,
    catalog: &Catalog,
    project_id: Option<&str>,
) -> DbResult<Vec<String>> {
    db.read(|c| {
        let mut dirs = Vec::new();
        for pkg in &catalog.packages {
            if let Some(sel) = effective(c, project_id, &pkg.manifest.id)? {
                let root = std::path::Path::new(&sel.install_path);
                for sub in pkg.adapter.path_dirs() {
                    dirs.push(
                        root.join(sub)
                            .to_string_lossy()
                            .trim_end_matches('\\')
                            .to_owned(),
                    );
                }
            }
        }
        Ok(dirs)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::domain::catalog::{
        Architecture, ValidationOptions, fixtures, parse_and_validate, version_row_id,
    };
    use crate::domain::operation::OperationStatus;
    use crate::persistence::projects::ProjectFields;
    use crate::persistence::test_support::db;
    use crate::services::journal::test_support::RecordingSink;

    /// Reports `fixture-tool <version>` based on which install folder the exe is in.
    struct VersionFromPath(Mutex<Vec<String>>);

    impl ExecutableProbe for VersionFromPath {
        fn run(&self, exe: &std::path::Path, _args: &[&str]) -> Result<String, String> {
            let dir = exe
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            let version = dir.split('-').next().unwrap().to_owned();
            if self.0.lock().unwrap().contains(&version) {
                return Err("crashed".into());
            }
            Ok(format!("fixture-tool {version}"))
        }
    }

    struct World {
        _dir: tempfile::TempDir,
        db: Database,
        catalog: Catalog,
        probe: Arc<dyn ExecutableProbe>,
        broken_versions: Arc<VersionFromPath>,
        sink: RecordingSink,
        v1: String,
        v2: String,
        a: String,
        b: String,
    }

    fn world() -> World {
        let catalog = parse_and_validate(
            &fixtures::document(vec![fixtures::package(
                "tool",
                vec![fixtures::version("1.0.0"), fixtures::version("2.0.0")],
            )]),
            ValidationOptions::default(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = db();
        let pkg = catalog.packages[0].clone();
        let mut ids = Vec::new();
        for v in &pkg.versions {
            let path = dir.path().join(format!("{}-x64", v.version));
            std::fs::create_dir_all(path.join("bin")).unwrap();
            std::fs::write(path.join("bin").join("tool.exe"), b"MZ").unwrap();
            let key = version_row_id("tool", &v.version, Architecture::X64);
            let id = db
                .write(|c| {
                    crate::persistence::packages::upsert(
                        c,
                        &crate::persistence::packages::fixtures::package("tool"),
                    )?;
                    crate::persistence::packages::insert_version(
                        c,
                        &crate::persistence::packages::fixtures::version(&key, "tool", &v.version),
                    )?;
                    let i = installations::begin(c, &key, &path.to_string_lossy())?;
                    installations::mark_installed(c, &i.id, "ok").map(|i| i.id)
                })
                .unwrap();
            ids.push(id);
        }
        let (a, b) = db
            .write(|c| {
                let a = projects::create(
                    c,
                    &ProjectFields {
                        name: "A".into(),
                        root_path: "D:\\a".into(),
                        ..Default::default()
                    },
                )?;
                let b = projects::create(
                    c,
                    &ProjectFields {
                        name: "B".into(),
                        root_path: "D:\\b".into(),
                        ..Default::default()
                    },
                )?;
                Ok((a.id, b.id))
            })
            .unwrap();
        let broken = Arc::new(VersionFromPath(Mutex::new(Vec::new())));
        World {
            _dir: dir,
            db,
            catalog,
            probe: broken.clone(),
            broken_versions: broken,
            sink: RecordingSink::default(),
            v1: ids[0].clone(),
            v2: ids[1].clone(),
            a,
            b,
        }
    }

    fn global(id: &str) -> SwitchRequest {
        SwitchRequest {
            package_id: "tool".into(),
            scope: Scope::Global,
            project_id: None,
            installation_id: Some(id.into()),
        }
    }

    fn pin(project: &str, id: Option<&str>) -> SwitchRequest {
        SwitchRequest {
            package_id: "tool".into(),
            scope: Scope::Project,
            project_id: Some(project.into()),
            installation_id: id.map(Into::into),
        }
    }

    fn eff(w: &World, project: Option<&str>) -> Option<String> {
        w.db.read(|c| effective(c, project, "tool"))
            .unwrap()
            .map(|s| s.version)
    }

    fn run(w: &World, req: &SwitchRequest) -> Result<Operation, IpcError> {
        apply(&w.db, &w.catalog, &w.probe, &w.sink, req)
    }

    #[test]
    fn two_versions_coexist_and_project_pins_are_isolated() {
        let w = world();
        assert_eq!(
            run(&w, &global(&w.v1)).unwrap().status,
            OperationStatus::Succeeded
        );
        assert_eq!(
            eff(&w, Some(&w.a)),
            Some("1.0.0".into()),
            "A inherits the default"
        );

        // Pin A to 2.0.0: B and the default are untouched.
        run(&w, &pin(&w.a, Some(&w.v2))).unwrap();
        assert_eq!(eff(&w, Some(&w.a)), Some("2.0.0".into()));
        assert_eq!(eff(&w, Some(&w.b)), Some("1.0.0".into()));
        assert_eq!(eff(&w, None), Some("1.0.0".into()));

        // Changing the default moves B (unpinned) but not A (pinned).
        run(&w, &global(&w.v2)).unwrap();
        run(&w, &pin(&w.a, Some(&w.v1))).unwrap();
        run(&w, &global(&w.v1)).unwrap();
        run(&w, &global(&w.v2)).unwrap();
        assert_eq!(eff(&w, Some(&w.a)), Some("1.0.0".into()));
        assert_eq!(eff(&w, Some(&w.b)), Some("2.0.0".into()));

        // Unpinning A makes it follow the default again.
        run(&w, &pin(&w.a, None)).unwrap();
        assert_eq!(eff(&w, Some(&w.a)), Some("2.0.0".into()));
    }

    #[test]
    fn preview_explains_scope_and_is_side_effect_free() {
        let w = world();
        run(&w, &global(&w.v1)).unwrap();
        let p = preview(&w.db, &w.catalog, &pin(&w.a, Some(&w.v2))).unwrap();
        assert_eq!(
            p.current.as_ref().unwrap().source,
            SelectionSource::GlobalDefault
        );
        assert_eq!(p.target.as_ref().unwrap().version, "2.0.0");
        assert!(
            p.changes[0].contains("A: TOOL 1.0.0 → TOOL 2.0.0"),
            "{:?}",
            p.changes
        );
        assert!(p.unchanged.iter().any(|u| u.contains("PATH")));
        assert_eq!(
            eff(&w, Some(&w.a)),
            Some("1.0.0".into()),
            "preview changes nothing"
        );

        let same = preview(&w.db, &w.catalog, &global(&w.v1)).unwrap();
        assert!(same.no_op);
        assert!(run(&w, &global(&w.v1)).is_err());
    }

    #[test]
    fn failed_verification_changes_nothing_and_is_journaled() {
        let w = world();
        run(&w, &global(&w.v1)).unwrap();
        w.broken_versions.0.lock().unwrap().push("2.0.0".into());

        let op = run(&w, &global(&w.v2)).unwrap();
        assert_eq!(op.status, OperationStatus::Failed);
        assert_eq!(op.error_code.as_deref(), Some("INTEGRITY_ERROR"));
        assert!(
            op.safe_error_message
                .unwrap()
                .contains("Could not run bin/tool.exe")
        );
        assert_eq!(eff(&w, None), Some("1.0.0".into()), "previous default kept");
    }

    #[test]
    fn missing_files_block_the_switch() {
        let w = world();
        let path =
            w.db.read(|c| installations::get(c, &w.v2))
                .unwrap()
                .install_path;
        std::fs::remove_file(std::path::Path::new(&path).join("bin").join("tool.exe")).unwrap();
        let op = run(&w, &pin(&w.a, Some(&w.v2))).unwrap();
        assert_eq!(op.status, OperationStatus::Failed);
        assert!(op.safe_error_message.unwrap().contains("missing"));
        assert_eq!(eff(&w, Some(&w.a)), None);
    }

    #[test]
    fn previous_selection_is_recorded_for_rollback() {
        let w = world();
        run(&w, &global(&w.v1)).unwrap();
        let op = run(&w, &global(&w.v2)).unwrap();
        assert_eq!(op.recovery_data["previousInstallationId"], json!(w.v1));
    }

    #[test]
    fn rejects_bad_requests() {
        let w = world();
        let not_installed =
            w.db.write(|c| {
                crate::persistence::packages::insert_version(
                    c,
                    &crate::persistence::packages::fixtures::version(
                        "tool@3.0.0@x64",
                        "tool",
                        "3.0.0",
                    ),
                )?;
                installations::begin(c, "tool@3.0.0@x64", "x").map(|i| i.id)
            })
            .unwrap();
        for req in [
            global(&not_installed),
            SwitchRequest {
                package_id: "other".into(),
                ..global(&w.v1)
            },
            SwitchRequest {
                project_id: None,
                ..pin(&w.a, Some(&w.v1))
            },
            SwitchRequest {
                installation_id: None,
                ..global(&w.v1)
            },
        ] {
            assert!(preview(&w.db, &w.catalog, &req).is_err(), "{req:?}");
        }
    }

    #[test]
    fn terminal_path_uses_effective_versions() {
        let w = world();
        run(&w, &global(&w.v1)).unwrap();
        run(&w, &pin(&w.a, Some(&w.v2))).unwrap();
        let a = project_path_prefix(&w.db, &w.catalog, &w.a).unwrap();
        let b = project_path_prefix(&w.db, &w.catalog, &w.b).unwrap();
        assert!(
            a[0].ends_with("2.0.0-x64") && b[0].ends_with("1.0.0-x64"),
            "{a:?} {b:?}"
        );
    }
}
