use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::error::IpcResult;
use crate::persistence::projects::{self, Project};
use crate::services::runtime::{self, Selection};
use crate::services::terminal::{self, TerminalOpened};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    #[serde(flatten)]
    pub project: Project,
    pub runtimes: Vec<ProjectRuntime>,
    /// File-based detection (type, suggested document root); nothing is executed.
    pub detection: crate::services::projects::Detection,
    /// Services the user chose for this project.
    pub service_ids: Vec<String>,
    /// What "Start all" starts: the chosen services plus what the project's sites need.
    pub start_service_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntime {
    pub package_id: String,
    pub package_name: String,
    /// What the project uses now (its pin or the global default), if anything.
    pub effective: Option<Selection>,
}

/// Projects with the runtime each one effectively uses, for runtime packages in the catalog.
#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> IpcResult<Vec<ProjectView>> {
    let (db, catalog) = (state.db()?, Arc::clone(&state.catalog));
    blocking(move || {
        Ok(db.read(|c| {
            projects::list(c)?
                .into_iter()
                .map(|project| {
                    let runtimes = catalog
                        .packages
                        .iter()
                        .filter(|p| {
                            p.manifest.category == crate::domain::catalog::Category::Runtime
                        })
                        .map(|p| {
                            Ok(ProjectRuntime {
                                package_id: p.manifest.id.clone(),
                                package_name: p.manifest.display_name.clone(),
                                effective: runtime::effective(
                                    c,
                                    Some(&project.id),
                                    &p.manifest.id,
                                )?,
                            })
                        })
                        .collect::<crate::persistence::DbResult<_>>()?;
                    let detection =
                        crate::services::projects::detect(std::path::Path::new(&project.root_path));
                    let service_ids = projects::service_ids(c, &project.id)?;
                    let start_service_ids =
                        crate::services::projects::services_to_start(c, &project.id)?
                            .into_iter()
                            .map(|s| s.id)
                            .collect();
                    Ok(ProjectView {
                        project,
                        runtimes,
                        detection,
                        service_ids,
                        start_service_ids,
                    })
                })
                .collect::<crate::persistence::DbResult<Vec<_>>>()
        })?)
    })
    .await
}

/// Adds `www\<folder>` as a project, creating the folder when it doesn't exist yet.
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    name: String,
    folder: String,
    service_ids: Vec<String>,
) -> IpcResult<Project> {
    let (db, paths) = (state.db()?, state.paths.clone());
    blocking(move || crate::services::projects::create(&db, &paths, &name, &folder, &service_ids))
        .await
}

/// Replaces the services a project uses.
#[tauri::command]
pub async fn set_project_services(
    state: State<'_, AppState>,
    project_id: String,
    service_ids: Vec<String>,
) -> IpcResult<()> {
    let db = state.db()?;
    blocking(move || Ok(db.write(|c| projects::set_service_ids(c, &project_id, &service_ids))?))
        .await
}

/// Starts every service the project needs that isn't running. Services whose port another
/// program holds are skipped and reported.
#[tauri::command]
pub async fn start_project_services(
    state: State<'_, AppState>,
    project_id: String,
) -> IpcResult<crate::services::projects::StartAll> {
    let (db, sup) = (state.db()?, state.supervisor()?);
    blocking(move || crate::services::projects::start_all(&db, &sup, &project_id)).await
}

/// Removes the project from Devluma (metadata, pins and links). The folder is not touched.
#[tauri::command]
pub async fn remove_project(state: State<'_, AppState>, project_id: String) -> IpcResult<()> {
    let db = state.db()?;
    blocking(move || Ok(db.write(|c| projects::remove(c, &project_id))?)).await
}

/// Opens the terminal chosen in Settings in the project folder, with its runtimes first on
/// PATH (that window only).
#[tauri::command]
pub async fn open_project_terminal(
    state: State<'_, AppState>,
    project_id: String,
) -> IpcResult<TerminalOpened> {
    let (db, catalog, paths) = (state.db()?, Arc::clone(&state.catalog), state.paths.clone());
    blocking(move || terminal::open(&db, &catalog, &paths, Some(&project_id))).await
}
