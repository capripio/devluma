//! Simple settings and the advanced config editor (ADR 0018).

use tauri::State;

use super::blocking;
use crate::error::IpcResult;
use crate::services::config::{
    ConfigApplied, ConfigChange, ConfigPreview, ConfigTarget, ConfigView,
};
use crate::state::AppState;

/// Settings, extensions (PHP) and config files of a PHP version or a service. Read-only.
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>, target: ConfigTarget) -> IpcResult<ConfigView> {
    let c = state.config()?;
    blocking(move || c.view(&target)).await
}

/// Exactly what saving would change (redacted diff) and what checks it. Read-only.
#[tauri::command]
pub async fn preview_config_change(
    state: State<'_, AppState>,
    target: ConfigTarget,
    change: ConfigChange,
) -> IpcResult<ConfigPreview> {
    let c = state.config()?;
    blocking(move || c.preview(&target, &change)).await
}

/// Backs up, writes, has the tool check it and rolls back on failure. Journaled.
#[tauri::command]
pub async fn apply_config_change(
    state: State<'_, AppState>,
    target: ConfigTarget,
    change: ConfigChange,
) -> IpcResult<ConfigApplied> {
    let c = state.config()?;
    blocking(move || c.apply(&target, &change)).await
}

/// A previous version of a config file, to load into the editor.
#[tauri::command]
pub async fn read_config_backup(
    state: State<'_, AppState>,
    target: ConfigTarget,
    file: String,
    backup_id: String,
) -> IpcResult<String> {
    let c = state.config()?;
    blocking(move || c.read_backup(&target, &file, &backup_id)).await
}
