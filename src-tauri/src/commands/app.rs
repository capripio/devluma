use serde::Serialize;
use tauri::{AppHandle, Runtime, State};

use crate::error::IpcResult;
use crate::state::{AppState, StartupStatus};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub debug_build: bool,
}

/// Static facts about this build. Cheap and side-effect free.
#[tauri::command]
pub fn get_app_info<R: Runtime>(app: AppHandle<R>) -> IpcResult<AppInfo> {
    let info = app.package_info();
    Ok(AppInfo {
        name: info.name.clone(),
        version: info.version.to_string(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        debug_build: cfg!(debug_assertions),
    })
}

/// Result of opening/migrating/reconciling the metadata database at launch. The UI shows a
/// recovery page instead of the app when `state` is `recovery_required`.
#[tauri::command]
pub fn get_startup_status(state: State<'_, AppState>) -> StartupStatus {
    state.startup.clone()
}
