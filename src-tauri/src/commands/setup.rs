use serde::Serialize;
use tauri::{AppHandle, Runtime, State};

use super::blocking;
use crate::error::IpcResult;
use crate::services::setup::{self, SetupDeps, SetupPlan, SetupStatus};
use crate::services::storage;
use crate::state::AppState;

fn storage_choice(state: &AppState) -> storage::StorageChoice {
    let mut choice = storage::resolve(cfg!(debug_assertions));
    choice.root = state.paths.root.clone();
    choice
}

#[tauri::command]
pub async fn get_setup_status(state: State<'_, AppState>) -> IpcResult<SetupStatus> {
    let (db, catalog, choice) = (state.db()?, state.catalog.clone(), storage_choice(&state));
    blocking(move || setup::status(&db, &catalog, &choice, cfg!(debug_assertions))).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageChanged {
    pub restart_needed: bool,
}

/// Keeps everything next to devluma.exe (`folder: null`) or in another folder, from the next
/// start. Existing data isn't moved.
#[tauri::command]
pub async fn choose_storage(
    state: State<'_, AppState>,
    folder: Option<String>,
) -> IpcResult<StorageChanged> {
    let choice = storage_choice(&state);
    blocking(move || {
        setup::choose_storage(&choice, cfg!(debug_assertions), folder.as_deref())
            .map(|restart_needed| StorageChanged { restart_needed })
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStarted {
    pub operation_id: String,
}

/// Installs the chosen packages, sets runtime defaults and creates services (one operation).
#[tauri::command]
pub async fn run_setup(state: State<'_, AppState>, plan: SetupPlan) -> IpcResult<SetupStarted> {
    let deps = SetupDeps {
        db: state.db()?,
        catalog: state.catalog.clone(),
        install_ctx: state.install_context()?,
        installs: state.installs.clone(),
        supervisor: state.supervisor()?,
        probe: state.probe(),
        sink: state.sink(),
    };
    let (operation_id, _handle) = setup::start(deps, plan)?;
    Ok(SetupStarted { operation_id })
}

#[tauri::command]
pub async fn complete_setup(state: State<'_, AppState>) -> IpcResult<()> {
    let db = state.db()?;
    blocking(move || setup::complete(&db)).await
}

#[tauri::command]
pub async fn reopen_setup(state: State<'_, AppState>) -> IpcResult<()> {
    let db = state.db()?;
    blocking(move || setup::reopen(&db)).await
}

/// Restarts Devluma (after choosing another storage folder). Services stop first.
#[tauri::command]
pub async fn restart_app<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> IpcResult<()> {
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
    app.restart();
}
