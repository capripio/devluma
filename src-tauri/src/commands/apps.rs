//! App installer and Composer (ADR 0020).

use tauri::State;

use super::blocking;
use crate::error::IpcResult;
use crate::services::apps::{self, AppInfo, AppPlan, AppProjectRequest, AppStarted};
use crate::services::composer::{Action, ComposerStatus};
use crate::state::AppState;

/// The apps a project can be created from, with what each needs.
#[tauri::command]
pub fn list_apps() -> Vec<AppInfo> {
    apps::catalog_of_apps()
}

/// What creating a project from an app would do. Changes nothing.
#[tauri::command]
pub async fn preview_app_project(
    state: State<'_, AppState>,
    request: AppProjectRequest,
) -> IpcResult<AppPlan> {
    let apps = state.apps()?;
    blocking(move || Ok(apps.preview(&request))).await
}

/// Creates a project from an app in the background. Returns the operation and project ids.
#[tauri::command]
pub async fn create_app_project(
    state: State<'_, AppState>,
    request: AppProjectRequest,
) -> IpcResult<AppStarted> {
    let (apps, ctx) = (state.apps()?, state.install_context()?);
    blocking(move || apps.start(ctx, &request)).await
}

#[tauri::command]
pub async fn get_composer_status(
    state: State<'_, AppState>,
    project_id: String,
) -> IpcResult<ComposerStatus> {
    let composer = state.composer()?;
    blocking(move || composer.status(&project_id)).await
}

/// `composer install` or `composer update` in the project folder. Returns the operation id.
#[tauri::command]
pub async fn run_composer(
    state: State<'_, AppState>,
    project_id: String,
    action: Action,
) -> IpcResult<String> {
    let composer = state.composer()?;
    blocking(move || composer.start(&project_id, action)).await
}

/// Turns on the PHP extensions Composer needs, for the project's PHP version.
#[tauri::command]
pub async fn enable_composer_extensions(
    state: State<'_, AppState>,
    project_id: String,
) -> IpcResult<()> {
    let composer = state.composer()?;
    blocking(move || composer.enable_extensions(&project_id)).await
}

/// pnpm / Yarn for one Node.js installation (through its Corepack).
#[tauri::command]
pub async fn get_node_tools(
    state: State<'_, AppState>,
    installation_id: String,
) -> IpcResult<crate::services::node_tools::NodeTools> {
    let db = state.db()?;
    blocking(move || crate::services::node_tools::status(&db, &installation_id)).await
}

#[tauri::command]
pub async fn set_node_tools(
    state: State<'_, AppState>,
    installation_id: String,
    enabled: Vec<crate::services::node_tools::PackageManager>,
) -> IpcResult<crate::services::node_tools::NodeTools> {
    let (db, paths) = (state.db()?, state.paths.clone());
    blocking(move || crate::services::node_tools::set(&db, &paths, &installation_id, &enabled))
        .await
}

/// Releases of an app to choose from (WordPress: the catalog; Laravel/Symfony: packagist.org).
/// `project_id`: judge PHP support by that project's PHP instead of the default.
#[tauri::command]
pub async fn list_app_versions(
    state: State<'_, AppState>,
    app: crate::services::apps::App,
    project_id: Option<String>,
) -> IpcResult<crate::services::apps::AppVersions> {
    let apps = state.apps()?;
    let a2 = std::sync::Arc::clone(&apps);
    let php = blocking(move || Ok(a2.php_for(project_id.as_deref()))).await?;
    Ok(apps.versions(app, php).await)
}
