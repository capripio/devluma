//! Thin Tauri command handlers: validate input, call domain services, return typed results.
//! Every command added here must also be listed in `build.rs` and granted in a capability.

pub mod app;
pub mod certificates;
pub mod databases;
pub mod diagnostics;
pub mod operations;
pub mod packages;
pub mod projects;
pub mod runtimes;
pub mod scheduler;
pub mod services;
pub mod settings;
pub mod setup;
pub mod sites;
pub mod tunnels;
pub mod updates;

use crate::error::{IpcError, IpcResult};
use crate::persistence::{Database, DbResult};
use crate::state::AppState;

/// Runs blocking database work off the async runtime and maps errors to safe IPC errors.
pub(crate) async fn with_db<T, F>(state: &AppState, f: F) -> IpcResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> DbResult<T> + Send + 'static,
{
    let db = state.db()?;
    tauri::async_runtime::spawn_blocking(move || f(&db))
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(IpcError::from)
}

/// Runs blocking work (DB, probes, file IO) off the async runtime.
pub(crate) async fn blocking<T, F>(f: F) -> IpcResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> IpcResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| IpcError::internal())?
}
