use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

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

/// The UI's close handler is mounted: from now on ✕ follows the user's close choice (ask,
/// keep running in the notification area, or quit with progress) instead of closing at once.
#[tauri::command]
pub fn register_close_handler<R: Runtime>(app: AppHandle<R>) {
    app.state::<crate::lifecycle::Lifecycle>().set_ui_ready();
}

/// Hides the window to the notification area; services keep running.
#[tauri::command]
pub fn hide_main_window<R: Runtime>(app: AppHandle<R>) {
    crate::lifecycle::hide_main(&app);
}

/// Stops every service (progress arrives as `service://state` events), then exits.
#[tauri::command]
pub fn quit_app<R: Runtime>(app: AppHandle<R>) {
    crate::lifecycle::begin_quit(&app);
}

/// "Quit anyway": exits now; services still running keep running and are re-adopted next time.
#[tauri::command]
pub fn quit_app_now<R: Runtime>(app: AppHandle<R>) {
    crate::lifecycle::quit_now(&app);
}

/// Result of opening/migrating/reconciling the metadata database at launch. The UI shows a
/// recovery page instead of the app when `state` is `recovery_required`.
#[tauri::command]
pub fn get_startup_status(state: State<'_, AppState>) -> StartupStatus {
    state.startup.clone()
}
