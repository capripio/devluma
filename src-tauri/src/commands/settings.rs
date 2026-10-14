//! Settings page (ADR 0015): preferences, start with Windows, the global terminal, storage.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::error::{ErrorCode, IpcError, IpcResult};
use crate::platform::windows::autostart::{LaunchAtLogin, RunKey};
use crate::services::preferences::{self, Preferences, SUGGESTED_SUFFIXES, SuffixAdvice};
use crate::services::storage::{self, FolderUsage};
use crate::services::terminal::{self, TerminalOpened};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesView {
    pub preferences: Preferences,
    /// Offered as suggestions; any valid suffix is accepted.
    pub suggested_suffixes: Vec<&'static str>,
    /// For the stored suffix.
    pub suffix_advice: SuffixAdvice,
    pub cmder_installed: bool,
    pub launch_at_login: LaunchAtLogin,
    /// False in development builds, which shouldn't register themselves to run at sign-in.
    pub launch_at_login_supported: bool,
}

fn view(db: &crate::persistence::Database) -> IpcResult<PreferencesView> {
    let (preferences, cmder) = db.read(|c| Ok((preferences::load(c)?, terminal::cmder_exe(c)?)))?;
    let launch_at_login = std::env::current_exe()
        .map(|exe| RunKey::current_user().status(&exe))
        .unwrap_or(LaunchAtLogin::Off);
    let suffix_advice = preferences::suffix_advice(&preferences.domain_suffix);
    Ok(PreferencesView {
        preferences,
        suggested_suffixes: SUGGESTED_SUFFIXES.to_vec(),
        suffix_advice,
        cmder_installed: cmder.is_some(),
        launch_at_login,
        launch_at_login_supported: !cfg!(debug_assertions),
    })
}

#[tauri::command]
pub async fn get_preferences(state: State<'_, AppState>) -> IpcResult<PreferencesView> {
    let db = state.db()?;
    blocking(move || view(&db)).await
}

/// Validates and stores all preferences at once; returns the stored result.
#[tauri::command]
pub async fn set_preferences(
    state: State<'_, AppState>,
    preferences: Preferences,
) -> IpcResult<PreferencesView> {
    let db = state.db()?;
    blocking(move || {
        db.write(|c| preferences::save(c, &preferences))?;
        view(&db)
    })
    .await
}

/// Adds or removes Devluma's entry in the current user's `Run` key.
#[tauri::command]
pub async fn set_launch_at_login(enabled: bool) -> IpcResult<LaunchAtLogin> {
    blocking(move || {
        if cfg!(debug_assertions) {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "Start with Windows is only available in installed builds.",
                false,
            ));
        }
        let exe = std::env::current_exe().map_err(|_| IpcError::internal())?;
        let run = RunKey::current_user();
        let result = if enabled {
            run.enable(&exe)
        } else {
            run.disable()
        };
        result.map_err(|_| {
            IpcError::new(
                ErrorCode::Internal,
                "Windows didn't accept the change to the startup programs.",
                true,
            )
        })?;
        Ok(run.status(&exe))
    })
    .await
}

/// Opens the preferred terminal in the user's home folder with the global default runtimes.
#[tauri::command]
pub async fn open_terminal(state: State<'_, AppState>) -> IpcResult<TerminalOpened> {
    let (db, catalog, paths) = (state.db()?, Arc::clone(&state.catalog), state.paths.clone());
    blocking(move || terminal::open(&db, &catalog, &paths, None)).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub root: String,
    pub folders: Vec<FolderUsage>,
}

#[tauri::command]
pub async fn get_storage_usage(state: State<'_, AppState>) -> IpcResult<StorageUsage> {
    state.db()?;
    let paths = state.paths.clone();
    blocking(move || {
        Ok(StorageUsage {
            root: paths.root.display().to_string(),
            folders: storage::usage(&paths),
        })
    })
    .await
}

/// Opens the storage root or one of the folders listed by `get_storage_usage`, by key.
#[tauri::command]
pub async fn open_storage_folder(state: State<'_, AppState>, key: String) -> IpcResult<()> {
    state.db()?;
    let paths = state.paths.clone();
    blocking(move || {
        let dir = if key == "root" {
            paths.root.clone()
        } else {
            storage::folders(&paths)
                .into_iter()
                .find(|(k, _)| *k == key)
                .map(|(_, path)| path)
                .ok_or_else(|| {
                    IpcError::new(ErrorCode::ValidationError, "Unknown folder.", false)
                })?
        };
        if !dir.is_dir() {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "That folder doesn't exist yet.",
                false,
            ));
        }
        crate::platform::windows::process::open_folder(&dir)
            .map_err(|_| IpcError::new(ErrorCode::Internal, "Could not open the folder.", true))
    })
    .await
}
