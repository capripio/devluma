use serde::Serialize;
use tauri::State;

use super::with_db;
use crate::domain::catalog::{
    Architecture, Catalog, CatalogIssue, Category, IntegrityPolicy, License, version_row_id,
};
use crate::error::IpcResult;
use crate::persistence::installations::{self, InstallStatus, Installation};
use crate::services::install::{InstallRequest, InstallStarted, install_dir};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    pub catalog_version: String,
    pub packages_dir: String,
    pub packages: Vec<PackageView>,
    pub issues: Vec<CatalogIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageView {
    pub id: String,
    pub display_name: String,
    pub category: Category,
    pub description: Option<String>,
    pub publisher: String,
    pub homepage_url: Option<String>,
    pub license: License,
    pub support_policy_url: Option<String>,
    pub versions: Vec<VersionView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    /// Matches `targetId` of this version's install operations.
    pub key: String,
    pub version: String,
    pub architecture: Architecture,
    pub size_bytes: u64,
    pub source_url: String,
    pub source_host: String,
    pub integrity_policy: IntegrityPolicy,
    pub sha256: String,
    pub dependencies: Vec<String>,
    pub conflicts: Vec<String>,
    pub release_date: Option<String>,
    pub end_of_life_date: Option<String>,
    pub install_path: String,
    /// Why this version can't be installed right now, if it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<InstallationView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationView {
    pub id: String,
    pub status: InstallStatus,
    pub install_path: String,
    pub is_global_default: bool,
    pub installed_at: Option<String>,
}

fn build_view(catalog: &Catalog, state: &AppState, installed: &[Installation]) -> CatalogView {
    let packages = catalog
        .packages
        .iter()
        .map(|p| {
            let m = &p.manifest;
            let versions = p
                .versions
                .iter()
                .map(|v| {
                    let key = version_row_id(&m.id, &v.version, v.architecture);
                    let installation = installed.iter().find(|i| i.package_version_id == key).map(|i| InstallationView {
                        id: i.id.clone(),
                        status: i.status,
                        install_path: i.install_path.clone(),
                        is_global_default: i.is_global_default,
                        installed_at: i.installed_at.clone(),
                    });
                    let unavailable_reason = if !v.architecture.runs_on_host() {
                        Some(format!("Built for {}; this computer can't run it.", v.architecture.as_str()))
                    } else if v.integrity.policy == IntegrityPolicy::Sha256AndSignature {
                        Some("Requires publisher-signature verification, which isn't available in this build.".into())
                    } else {
                        None
                    };
                    VersionView {
                        key: key.clone(),
                        version: v.version.clone(),
                        architecture: v.architecture,
                        size_bytes: v.source.size_bytes,
                        source_url: v.source.url.clone(),
                        source_host: url::Url::parse(&v.source.url)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_owned))
                            .unwrap_or_default(),
                        integrity_policy: v.integrity.policy,
                        sha256: v.integrity.sha256.clone(),
                        dependencies: v.dependencies.clone(),
                        conflicts: v.conflicts.clone(),
                        release_date: v.release_date.clone(),
                        end_of_life_date: v.end_of_life_date.clone(),
                        install_path: install_dir(&state.paths, &m.id, &v.version, v.architecture)
                            .display()
                            .to_string(),
                        unavailable_reason,
                        installation,
                    }
                })
                .collect();
            PackageView {
                id: m.id.clone(),
                display_name: m.display_name.clone(),
                category: m.category,
                description: m.description.clone(),
                publisher: m.publisher.clone(),
                homepage_url: m.homepage_url.clone(),
                license: m.license.clone(),
                support_policy_url: m.support_policy_url.clone(),
                versions,
            }
        })
        .collect();
    CatalogView {
        catalog_version: catalog.catalog_version.clone(),
        packages_dir: state.paths.packages.display().to_string(),
        packages,
        issues: catalog.issues.clone(),
    }
}

/// Catalog entries merged with what's installed.
#[tauri::command]
pub async fn list_packages(state: State<'_, AppState>) -> IpcResult<CatalogView> {
    let installed = with_db(&state, |db| db.read(installations::list)).await?;
    // Removed rows are history, not state.
    let installed: Vec<_> = installed
        .into_iter()
        .filter(|i| i.status != InstallStatus::Removed)
        .collect();
    Ok(build_view(&state.catalog, &state, &installed))
}

/// Starts an install and returns its operation id immediately; progress arrives via
/// `operation://progress`.
#[tauri::command]
pub async fn install_package(
    state: State<'_, AppState>,
    request: InstallRequest,
) -> IpcResult<InstallStarted> {
    let ctx = state.install_context()?;
    let installs = state.installs.clone();
    let (started, _handle) = installs.start(ctx, request)?;
    Ok(started)
}

/// What uninstalling a version would do (blocked by services/sites; data is kept).
#[tauri::command]
pub async fn preview_uninstall(
    state: State<'_, AppState>,
    installation_id: String,
) -> IpcResult<crate::services::uninstall::UninstallPlan> {
    let db = state.db()?;
    super::blocking(move || crate::services::uninstall::preview(&db, &installation_id)).await
}

/// Deletes a version's package folder. Data folders, projects and backups are kept.
#[tauri::command]
pub async fn uninstall_package(
    state: State<'_, AppState>,
    installation_id: String,
) -> IpcResult<crate::persistence::operations::Operation> {
    let (db, sink, packages) = (state.db()?, state.sink(), state.paths.packages.clone());
    super::blocking(move || {
        crate::services::uninstall::uninstall(&db, sink.as_ref(), &packages, &installation_id)
    })
    .await
}
