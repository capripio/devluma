use tauri::State;

use super::blocking;
use crate::error::{ErrorCode, IpcError, IpcResult};
use crate::services::diagnostics::DiagnosticsPreview;
use crate::state::AppState;

/// Exactly what an export would contain (redacted; personal folders masked by default).
#[tauri::command]
pub async fn preview_diagnostics(
    state: State<'_, AppState>,
    mask_paths: bool,
) -> IpcResult<DiagnosticsPreview> {
    let d = state.diagnostics()?;
    blocking(move || {
        d.preview(mask_paths)
            .map_err(|f| IpcError::new(f.code, f.message, true))
    })
    .await
}

/// Writes the previewed sections to a zip in Devluma's diagnostics folder; returns its path.
#[tauri::command]
pub async fn export_diagnostics(state: State<'_, AppState>, mask_paths: bool) -> IpcResult<String> {
    let d = state.diagnostics()?;
    blocking(move || {
        d.export(mask_paths)
            .map(|p| p.display().to_string())
            .map_err(|f| IpcError::new(f.code, f.message, true))
    })
    .await
}

#[tauri::command]
pub async fn open_diagnostics_folder(state: State<'_, AppState>) -> IpcResult<()> {
    let d = state.diagnostics()?;
    blocking(move || {
        let dir = d.export_dir();
        let _ = std::fs::create_dir_all(&dir);
        crate::platform::windows::process::open_folder(&dir)
            .map_err(|_| IpcError::new(ErrorCode::Internal, "Could not open the folder.", true))
    })
    .await
}
