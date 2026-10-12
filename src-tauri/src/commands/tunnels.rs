use tauri::State;

use super::blocking;
use crate::error::{IpcError, IpcResult};
use crate::services::supervisor::behavior::Failure;
use crate::services::tunnels::{NgrokStatus, TunnelView};
use crate::state::AppState;

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

/// Which ngrok.exe is used, whether a token is saved (never the token itself), and any
/// ngrok.exe found in common places.
#[tauri::command]
pub async fn get_ngrok_status(state: State<'_, AppState>) -> IpcResult<NgrokStatus> {
    let t = state.tunnels()?;
    blocking(move || t.status().map_err(ipc)).await
}

#[tauri::command]
pub async fn set_ngrok_path(state: State<'_, AppState>, path: String) -> IpcResult<NgrokStatus> {
    let t = state.tunnels()?;
    blocking(move || t.set_path(&path).map_err(ipc)).await
}

/// Stores the authtoken in Windows Credential Manager. It is never sent back to the UI.
#[tauri::command]
pub async fn set_ngrok_token(state: State<'_, AppState>, token: String) -> IpcResult<NgrokStatus> {
    let t = state.tunnels()?;
    blocking(move || t.set_token(&token).map_err(ipc)).await
}

#[tauri::command]
pub async fn remove_ngrok_token(state: State<'_, AppState>) -> IpcResult<NgrokStatus> {
    let t = state.tunnels()?;
    blocking(move || t.remove_token().map_err(ipc)).await
}

#[tauri::command]
pub async fn list_tunnels(state: State<'_, AppState>) -> IpcResult<Vec<TunnelView>> {
    let t = state.tunnels()?;
    blocking(move || Ok(t.list())).await
}

/// Makes a site reachable from the internet through ngrok (journaled).
#[tauri::command]
pub async fn start_tunnel(state: State<'_, AppState>, site_id: String) -> IpcResult<TunnelView> {
    let t = state.tunnels()?;
    blocking(move || t.start(&site_id).map_err(ipc)).await
}

#[tauri::command]
pub async fn stop_tunnel(state: State<'_, AppState>, site_id: String) -> IpcResult<()> {
    let t = state.tunnels()?;
    blocking(move || t.stop(&site_id).map_err(ipc)).await
}
