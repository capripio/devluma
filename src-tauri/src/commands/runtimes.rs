use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use super::blocking;
use crate::error::{ErrorCode, IpcError, IpcResult};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::NewOperation;
use crate::security::paths;
use crate::services::journal::{Journal, OperationProgress};
use crate::services::php::{self, IniTemplate, PhpDetails};
use crate::services::runtime::{self, SwitchPreview, SwitchRequest};
use crate::state::AppState;

/// Exactly what a runtime switch would change. Read-only.
#[tauri::command]
pub async fn preview_runtime_switch(
    state: State<'_, AppState>,
    request: SwitchRequest,
) -> IpcResult<SwitchPreview> {
    let (db, catalog) = (state.db()?, Arc::clone(&state.catalog));
    blocking(move || runtime::preview(&db, &catalog, &request)).await
}

/// Verifies the target executables, then applies the switch transactionally. Returns the
/// journaled operation (`succeeded` or `failed` with a safe message).
#[tauri::command]
pub async fn apply_runtime_switch(
    state: State<'_, AppState>,
    request: SwitchRequest,
) -> IpcResult<OperationProgress> {
    let (db, catalog, probe, sink) = (
        state.db()?,
        Arc::clone(&state.catalog),
        state.probe(),
        state.sink(),
    );
    blocking(move || {
        runtime::apply(&db, &catalog, &probe, sink.as_ref(), &request)
            .map(|op| OperationProgress::from(&op))
    })
    .await
}

/// Resolves an installed PHP's folder, refusing anything that isn't a verified PHP install
/// under Devluma's packages directory.
fn php_install_dir(state: &AppState, installation_id: &str) -> IpcResult<PathBuf> {
    let inst = state
        .db()?
        .read(|c| installations::get(c, installation_id))?;
    if inst.package_id != "php" || inst.status != InstallStatus::Installed {
        return Err(IpcError::new(
            ErrorCode::ValidationError,
            "That isn't an installed PHP version.",
            false,
        ));
    }
    paths::ensure_within(
        &state.paths.packages,
        std::path::Path::new(&inst.install_path),
    )
    .map_err(|_| {
        IpcError::new(
            ErrorCode::ValidationError,
            "This PHP installation's folder is missing or outside the packages directory.",
            false,
        )
    })
}

#[tauri::command]
pub async fn get_php_details(
    state: State<'_, AppState>,
    installation_id: String,
) -> IpcResult<PhpDetails> {
    let dir = php_install_dir(&state, &installation_id)?;
    let probe = state.probe();
    blocking(move || {
        php::inspect(probe.as_ref(), &dir).map_err(|e| {
            IpcError::new(
                ErrorCode::ProcessExited,
                format!("Could not run php.exe: {e}"),
                true,
            )
        })
    })
    .await
}

/// Creates php.ini from an official template (never overwrites). Journaled.
#[tauri::command]
pub async fn create_php_ini(
    state: State<'_, AppState>,
    installation_id: String,
    template: IniTemplate,
) -> IpcResult<PhpDetails> {
    let dir = php_install_dir(&state, &installation_id)?;
    let (db, probe, sink) = (state.db()?, state.probe(), state.sink());
    blocking(move || {
        let journal = Journal::new(&db, sink.as_ref());
        let op = journal.start(&NewOperation {
            operation_type: "create_php_ini",
            target_type: Some("package_installation"),
            target_id: Some(&installation_id),
        })?;
        journal.running(&op.id)?;
        match php::create_ini(probe.as_ref(), &dir, template) {
            Ok(_) => {
                journal.succeed(&op.id)?;
                php::inspect(probe.as_ref(), &dir)
                    .map_err(|e| IpcError::new(ErrorCode::ProcessExited, e, true))
            }
            Err(e) => {
                let code = match e {
                    php::IniError::Invalid(_) => ErrorCode::ConfigInvalid,
                    php::IniError::Io(_) => ErrorCode::Internal,
                    _ => ErrorCode::ValidationError,
                };
                journal.fail(&op.id, code, &e.to_string())?;
                Err(IpcError::new(code, e.to_string(), false))
            }
        }
    })
    .await
}
