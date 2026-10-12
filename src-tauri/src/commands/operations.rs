use tauri::State;

use super::with_db;
use crate::error::{ErrorCode, IpcError, IpcResult};
use crate::persistence::operations;
use crate::services::journal::OperationProgress;
use crate::state::AppState;

/// How many finished operations to include alongside all unfinished ones.
const RECENT_FINISHED: u32 = 20;

/// Unfinished operations (including `needs_recovery`) plus recent finished ones, newest first.
#[tauri::command]
pub async fn list_operations(state: State<'_, AppState>) -> IpcResult<Vec<OperationProgress>> {
    with_db(&state, |db| {
        let ops = db.read(|c| operations::list_for_display(c, RECENT_FINISHED))?;
        Ok(ops.iter().map(OperationProgress::from).collect())
    })
    .await
}

/// Requests cancellation of a running operation. The operation reports `cancelled` once
/// it has stopped and cleaned up.
#[tauri::command]
pub fn cancel_operation(state: State<'_, AppState>, operation_id: String) -> IpcResult<()> {
    if state.installs.cancel(&operation_id) {
        Ok(())
    } else {
        Err(IpcError::new(
            ErrorCode::ValidationError,
            "That operation isn't running, so there's nothing to cancel.",
            false,
        ))
    }
}
