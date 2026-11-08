//! FrankenPHP from the user's own frankenphp.exe (ADR 0019; `services::frankenphp`).

use tauri::State;

use super::blocking;
use crate::error::IpcResult;
use crate::services::frankenphp::{self, FrankenphpStatus};
use crate::state::AppState;

/// Which frankenphp.exe is used, its FrankenPHP and PHP versions, the services using it and
/// any frankenphp.exe found in common places.
#[tauri::command]
pub async fn get_frankenphp_status(state: State<'_, AppState>) -> IpcResult<FrankenphpStatus> {
    let db = state.db()?;
    blocking(move || frankenphp::status(&db)).await
}

/// Checks and records the user's frankenphp.exe (never downloaded or modified by Devluma).
#[tauri::command]
pub async fn set_frankenphp_path(
    state: State<'_, AppState>,
    path: String,
) -> IpcResult<FrankenphpStatus> {
    let db = state.db()?;
    blocking(move || frankenphp::set_path(&db, &path)).await
}

#[tauri::command]
pub async fn remove_frankenphp(state: State<'_, AppState>) -> IpcResult<FrankenphpStatus> {
    let db = state.db()?;
    blocking(move || frankenphp::remove(&db)).await
}
