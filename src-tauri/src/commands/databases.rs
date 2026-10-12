use tauri::State;

use super::blocking;
use crate::error::{IpcError, IpcResult};
use crate::persistence::backups::BackupRecord;
use crate::services::databases::{DatabaseServiceView, RestorePlan, RestoreRequest};
use crate::services::supervisor::behavior::Failure;
use crate::state::AppState;

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

#[tauri::command]
pub async fn list_database_services(
    state: State<'_, AppState>,
) -> IpcResult<Vec<DatabaseServiceView>> {
    let d = state.databases()?;
    blocking(move || d.list().map_err(ipc)).await
}

/// User databases on a running server.
#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<Vec<String>> {
    let d = state.databases()?;
    blocking(move || d.databases(&service_id).map_err(ipc)).await
}

#[tauri::command]
pub async fn list_database_backups(state: State<'_, AppState>) -> IpcResult<Vec<BackupRecord>> {
    let d = state.databases()?;
    blocking(move || d.backups().map_err(ipc)).await
}

/// Logical backup of one database (journaled).
#[tauri::command]
pub async fn backup_database(
    state: State<'_, AppState>,
    service_id: String,
    database: String,
) -> IpcResult<BackupRecord> {
    let d = state.databases()?;
    blocking(move || d.backup_database(&service_id, &database).map_err(ipc)).await
}

/// Copy of a stopped service's data folder (journaled).
#[tauri::command]
pub async fn snapshot_database(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<BackupRecord> {
    let d = state.databases()?;
    blocking(move || d.snapshot(&service_id).map_err(ipc)).await
}

#[tauri::command]
pub async fn preview_restore(
    state: State<'_, AppState>,
    request: RestoreRequest,
) -> IpcResult<RestorePlan> {
    let d = state.databases()?;
    blocking(move || d.preview_restore(&request).map_err(ipc)).await
}

/// Restores a backup. Refused unless an overwrite shown in the preview is confirmed.
#[tauri::command]
pub async fn restore_database(
    state: State<'_, AppState>,
    request: RestoreRequest,
) -> IpcResult<RestorePlan> {
    let d = state.databases()?;
    blocking(move || d.restore(&request).map_err(ipc)).await
}
