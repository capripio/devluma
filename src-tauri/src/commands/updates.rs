use serde::Serialize;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_updater::UpdaterExt;

use super::blocking;
use crate::error::{ErrorCode, IpcError, IpcResult};
use crate::services::supervisor::behavior::Failure;
use crate::services::updates::{CatalogCheck, UpdateSettings};
use crate::state::AppState;

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

#[tauri::command]
pub async fn get_update_settings(state: State<'_, AppState>) -> IpcResult<UpdateSettings> {
    let u = state.updates()?;
    blocking(move || u.settings().map_err(ipc)).await
}

/// Sets (or clears, with "") the app-update and catalog-update URLs. HTTPS only.
#[tauri::command]
pub async fn set_update_urls(
    state: State<'_, AppState>,
    app_url: String,
    catalog_url: String,
) -> IpcResult<UpdateSettings> {
    let u = state.updates()?;
    blocking(move || u.set_urls(&app_url, &catalog_url).map_err(ipc)).await
}

/// Downloads `catalog.json` + `.sig`, verifies and stores a newer catalog for the next start.
#[tauri::command]
pub async fn check_catalog_update(state: State<'_, AppState>) -> IpcResult<CatalogCheck> {
    let u = state.updates()?;
    u.check_catalog_update().await.map_err(ipc)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdate {
    pub current_version: String,
    pub available_version: Option<String>,
    pub notes: Option<String>,
    pub date: Option<String>,
}

fn updater<R: Runtime>(app: &AppHandle<R>, url: &str) -> IpcResult<tauri_plugin_updater::Updater> {
    let fail = |e: String| IpcError::new(ErrorCode::Internal, format!("Updater error: {e}"), true);
    let url: url::Url = url.parse().map_err(|_| {
        IpcError::new(
            ErrorCode::ValidationError,
            "The update URL is invalid.",
            false,
        )
    })?;
    app.updater_builder()
        .endpoints(vec![url])
        .map_err(|e| fail(e.to_string()))?
        .build()
        .map_err(|e| fail(e.to_string()))
}

fn configured_url(state: &AppState) -> IpcResult<String> {
    state
        .updates()?
        .settings()
        .map_err(ipc)?
        .app_url
        .ok_or_else(|| {
            IpcError::new(
                ErrorCode::NotImplemented,
                "App updates aren't configured for this build.",
                false,
            )
        })
}

/// Asks the update server whether a newer signed release exists. Nothing is downloaded.
#[tauri::command]
pub async fn check_app_update<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> IpcResult<AppUpdate> {
    let url = configured_url(&state)?;
    let found = updater(&app, &url)?.check().await.map_err(|e| {
        IpcError::new(
            ErrorCode::NetworkError,
            format!("Could not check for updates: {e}"),
            true,
        )
    })?;
    Ok(AppUpdate {
        current_version: app.package_info().version.to_string(),
        available_version: found.as_ref().map(|u| u.version.clone()),
        notes: found.as_ref().and_then(|u| u.body.clone()),
        date: found.as_ref().and_then(|u| u.date.map(|d| d.to_string())),
    })
}

/// Downloads the update, verifies its signature (the plugin rejects unsigned or wrongly signed
/// files), stops tunnels and services, installs and restarts Devluma.
#[tauri::command]
pub async fn install_app_update<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> IpcResult<()> {
    let url = configured_url(&state)?;
    let update = updater(&app, &url)?
        .check()
        .await
        .map_err(|e| {
            IpcError::new(
                ErrorCode::NetworkError,
                format!("Could not check for updates: {e}"),
                true,
            )
        })?
        .ok_or_else(|| {
            IpcError::new(ErrorCode::ValidationError, "No update is available.", false)
        })?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| {
        IpcError::new(
            ErrorCode::IntegrityError,
            format!("The update was rejected: {e}"),
            true,
        )
    })?;
    // The installer replaces Devluma's files; stop what Devluma owns first.
    let (tunnels, sup) = (state.tunnels.clone(), state.supervisor.clone());
    blocking(move || {
        if let Some(t) = tunnels {
            t.stop_all();
        }
        if let Some(s) = sup {
            s.stop_all_blocking();
        }
        Ok(())
    })
    .await?;
    update.install(bytes).map_err(|e| {
        IpcError::new(
            ErrorCode::Internal,
            format!("Could not start the installer: {e}"),
            true,
        )
    })?;
    app.restart();
}
