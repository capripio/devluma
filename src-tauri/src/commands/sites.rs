use serde::{Deserialize, Serialize};
use tauri::State;

use super::blocking;
use crate::error::{ErrorCode, IpcError, IpcResult};
use crate::persistence::operations::NewOperation;
use crate::services::hosts::{HostsPlan, HostsView};
use crate::services::journal::Journal;
use crate::services::sites::{SiteCheck, SitePlan, SiteRequest, SiteView};
use crate::services::supervisor::behavior::Failure;
use crate::state::AppState;

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStarted {
    pub operation_id: String,
}

#[tauri::command]
pub async fn list_sites(state: State<'_, AppState>) -> IpcResult<Vec<SiteView>> {
    let sites = state.sites()?;
    blocking(move || sites.list()).await
}

/// Generated config, its diff and the hosts diff. Changes nothing.
#[tauri::command]
pub async fn preview_site(state: State<'_, AppState>, request: SiteRequest) -> IpcResult<SitePlan> {
    let sites = state.sites()?;
    blocking(move || sites.preview(&request).map_err(ipc)).await
}

/// Applies a previewed site in the background (may show a Windows administrator prompt
/// for the hosts file). Returns the operation id.
#[tauri::command]
pub async fn apply_site(
    state: State<'_, AppState>,
    request: SiteRequest,
) -> IpcResult<OperationStarted> {
    let sites = state.sites()?;
    blocking(move || {
        sites
            .apply(request)
            .map(|(operation_id, _)| OperationStarted { operation_id })
    })
    .await
}

#[tauri::command]
pub async fn remove_site(
    state: State<'_, AppState>,
    site_id: String,
) -> IpcResult<OperationStarted> {
    let sites = state.sites()?;
    blocking(move || {
        sites
            .remove(&site_id)
            .map(|(operation_id, _)| OperationStarted { operation_id })
    })
    .await
}

#[tauri::command]
pub async fn check_site(state: State<'_, AppState>, site_id: String) -> IpcResult<SiteCheck> {
    let sites = state.sites()?;
    blocking(move || Ok(sites.check_site(&site_id))).await
}

/// Opens the site in the default browser (only URLs of existing sites).
#[tauri::command]
pub async fn open_site(state: State<'_, AppState>, site_id: String) -> IpcResult<String> {
    let sites = state.sites()?;
    blocking(move || sites.open(&site_id)).await
}

#[tauri::command]
pub async fn get_hosts(state: State<'_, AppState>) -> IpcResult<HostsView> {
    let hosts = state.hosts()?;
    blocking(move || hosts.view().map_err(ipc)).await
}

/// A change to Devluma's hosts entries: remove some, or put back the set from a backup.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsChangeRequest {
    #[serde(default)]
    pub remove_domains: Vec<String>,
    #[serde(default)]
    pub restore_backup_id: Option<String>,
    /// From the preview; apply refuses if the file changed since.
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

fn plan_for(
    hosts: &crate::services::hosts::HostsService,
    req: &HostsChangeRequest,
) -> Result<HostsPlan, Failure> {
    match &req.restore_backup_id {
        Some(id) => hosts.plan(&hosts.entries_in_backup(id)?),
        None => hosts.plan_change(&[], &req.remove_domains),
    }
}

#[tauri::command]
pub async fn preview_hosts_change(
    state: State<'_, AppState>,
    request: HostsChangeRequest,
) -> IpcResult<HostsPlan> {
    let hosts = state.hosts()?;
    blocking(move || plan_for(&hosts, &request).map_err(ipc)).await
}

/// Applies a previewed hosts change as a journaled operation (may prompt for admin).
#[tauri::command]
pub async fn apply_hosts_change(
    state: State<'_, AppState>,
    request: HostsChangeRequest,
) -> IpcResult<OperationStarted> {
    let (hosts, db, sink) = (state.hosts()?, state.db()?, state.sink());
    blocking(move || {
        let plan = plan_for(&hosts, &request).map_err(ipc)?;
        if request
            .expected_sha256
            .as_ref()
            .is_some_and(|s| s != &plan.sha256_before)
        {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "The hosts file changed since the preview. Review the change again.",
                true,
            ));
        }
        let journal = Journal::new(&db, sink.as_ref());
        let op = journal.start(&NewOperation {
            operation_type: "apply_hosts",
            target_type: Some("hosts_file"),
            target_id: None,
        })?;
        let op_id = op.id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let journal = Journal::new(&db, sink.as_ref());
            let _ = journal.running(&op_id);
            let reason = if request.restore_backup_id.is_some() {
                "Restore from backup"
            } else {
                "Remove entries"
            };
            let _ = match hosts.apply(&plan, reason, &op_id) {
                Ok(_) => journal.succeed(&op_id).map(|_| ()),
                Err(f) => journal.fail(&op_id, f.code, &f.message).map(|_| ()),
            };
        });
        Ok(OperationStarted {
            operation_id: op.id,
        })
    })
    .await
}
