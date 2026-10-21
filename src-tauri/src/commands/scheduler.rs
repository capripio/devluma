//! Scheduler page (ADR 0017): recurring project tasks Devluma runs while it is open.

use tauri::State;

use super::blocking;
use crate::error::{IpcError, IpcResult};
use crate::persistence::scheduler::Run;
use crate::services::scheduler::{JobPreview, JobRequest, JobView};
use crate::services::supervisor::behavior::Failure;
use crate::state::AppState;

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

#[tauri::command]
pub async fn list_scheduled_jobs(state: State<'_, AppState>) -> IpcResult<Vec<JobView>> {
    let s = state.scheduler()?;
    blocking(move || s.list().map_err(ipc)).await
}

/// Validates a job and returns exactly what it would run and its next run times, for the
/// confirmation shown before saving. Nothing is stored or run.
#[tauri::command]
pub async fn preview_scheduled_job(
    state: State<'_, AppState>,
    request: JobRequest,
) -> IpcResult<JobPreview> {
    let s = state.scheduler()?;
    blocking(move || s.preview(&request).map_err(ipc)).await
}

#[tauri::command]
pub async fn save_scheduled_job(
    state: State<'_, AppState>,
    request: JobRequest,
) -> IpcResult<JobView> {
    let s = state.scheduler()?;
    blocking(move || s.save(&request).map_err(ipc)).await
}

/// Mute (`false`) or unmute (`true`) a job; muted jobs keep their history and never run.
#[tauri::command]
pub async fn set_scheduled_job_enabled(
    state: State<'_, AppState>,
    job_id: String,
    enabled: bool,
) -> IpcResult<JobView> {
    let s = state.scheduler()?;
    blocking(move || s.set_enabled(&job_id, enabled).map_err(ipc)).await
}

#[tauri::command]
pub async fn run_scheduled_job_now(state: State<'_, AppState>, job_id: String) -> IpcResult<Run> {
    let s = state.scheduler()?;
    blocking(move || s.run_now(&job_id).map_err(ipc)).await
}

#[tauri::command]
pub async fn stop_scheduled_job_run(state: State<'_, AppState>, job_id: String) -> IpcResult<()> {
    let s = state.scheduler()?;
    blocking(move || s.stop(&job_id).map_err(ipc)).await
}

#[tauri::command]
pub async fn delete_scheduled_job(state: State<'_, AppState>, job_id: String) -> IpcResult<()> {
    let s = state.scheduler()?;
    blocking(move || s.delete(&job_id).map_err(ipc)).await
}

#[tauri::command]
pub async fn list_scheduled_job_runs(
    state: State<'_, AppState>,
    job_id: String,
) -> IpcResult<Vec<Run>> {
    let s = state.scheduler()?;
    blocking(move || s.runs(&job_id).map_err(ipc)).await
}

/// The end of one run's output, with secrets redacted.
#[tauri::command]
pub async fn get_scheduled_job_run_log(
    state: State<'_, AppState>,
    job_id: String,
    run_id: String,
) -> IpcResult<String> {
    let s = state.scheduler()?;
    blocking(move || s.run_log(&job_id, &run_id).map_err(ipc)).await
}
