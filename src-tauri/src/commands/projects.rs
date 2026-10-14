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
                    Ok(ProjectView {
                        project,
                        runtimes,
                        detection,
                    })
                })
                .collect::<crate::persistence::DbResult<Vec<_>>>()
        })?)
    })
    .await
}

#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    name: String,
    root_path: String,
) -> IpcResult<Project> {
    let (db, paths) = (state.db()?, state.paths.clone());
    blocking(move || crate::services::projects::create(&db, &paths, &name, &root_path)).await
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
