use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::domain::adapter::{AdapterId, ServiceRole};
use crate::error::IpcResult;
use crate::persistence::installations;
use crate::persistence::services::{self, ServiceInstance};
use crate::platform::windows::ports;
use crate::services::supervisor::{CreateServiceRequest, ServiceDetails};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    #[serde(flatten)]
    pub service: ServiceInstance,
    pub role: Option<ServiceRole>,
    pub package_id: String,
    pub package_name: String,
    pub version: String,
    /// For a service that isn't running: its ports another program is already using, so it
    /// can't start until that program stops or the port changes.
    pub port_conflicts: Vec<PortConflict>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortConflict {
    pub port: u16,
    /// "httpd.exe (PID 1234)".
    pub used_by: String,
}

/// Ports of a stopped service that another program is listening on.
fn port_conflicts(service: &ServiceInstance) -> Vec<PortConflict> {
    if service.observed_state.is_live() {
        return Vec::new();
    }
    [service.port, service.admin_port]
        .into_iter()
        .flatten()
        .filter_map(|p| u16::try_from(p).ok())
        .filter_map(|port| ports::used_by(port).map(|used_by| PortConflict { port, used_by }))
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStarted {
    pub operation_id: String,
}

#[tauri::command]
pub async fn list_services(state: State<'_, AppState>) -> IpcResult<Vec<ServiceView>> {
    let (db, catalog) = (state.db()?, Arc::clone(&state.catalog));
    blocking(move || {
        let (all, insts) = db.read(|c| Ok((services::list(c)?, installations::list(c)?)))?;
        Ok(all
            .into_iter()
            .map(|service| {
                let inst = insts
                    .iter()
                    .find(|i| i.id == service.package_installation_id);
                let package_id = inst.map(|i| i.package_id.clone()).unwrap_or_default();
                let version = inst
                    .and_then(|i| i.package_version_id.split('@').nth(1))
                    .unwrap_or_default()
                    .to_owned();
                let package_name = catalog
                    .package(&package_id)
                    .map_or_else(|| package_id.clone(), |p| p.manifest.display_name.clone());
                ServiceView {
                    role: AdapterId::parse(&service.lifecycle_adapter)
                        .and_then(AdapterId::service_role),
                    port_conflicts: port_conflicts(&service),
                    service,
                    package_id,
                    package_name,
                    version,
                }
            })
            .collect())
    })
    .await
}

/// Creates a service from an installed web server or database (initializing a new database
/// data folder when needed). Runs in the background; returns the operation id.
#[tauri::command]
pub async fn create_service(
    state: State<'_, AppState>,
    request: CreateServiceRequest,
) -> IpcResult<OperationStarted> {
    let sup = state.supervisor()?;
    blocking(move || {
        sup.create(request)
            .map(|(operation_id, _)| OperationStarted { operation_id })
    })
    .await
}

#[tauri::command]
pub async fn start_service(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<OperationStarted> {
    let sup = state.supervisor()?;
    blocking(move || {
        sup.start(&service_id)
            .map(|(operation_id, _)| OperationStarted { operation_id })
    })
    .await
}

#[tauri::command]
pub async fn stop_service(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<OperationStarted> {
    let sup = state.supervisor()?;
    blocking(move || {
        sup.stop(&service_id)
            .map(|(operation_id, _)| OperationStarted { operation_id })
    })
    .await
}

#[tauri::command]
pub async fn restart_service(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<OperationStarted> {
    let sup = state.supervisor()?;
    blocking(move || {
        sup.restart(&service_id)
            .map(|(operation_id, _)| OperationStarted { operation_id })
    })
    .await
}

/// Configuration and recent log lines (redacted).
#[tauri::command]
pub async fn get_service_details(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<ServiceDetails> {
    let sup = state.supervisor()?;
    blocking(move || sup.details(&service_id)).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRemoved {
    /// Data folder that was kept, if the service had one.
    pub kept_data_path: Option<String>,
}

/// Removes a stopped service and its Devluma-managed folder. Data folders are never deleted.
#[tauri::command]
pub async fn remove_service(
    state: State<'_, AppState>,
    service_id: String,
) -> IpcResult<ServiceRemoved> {
    let sup = state.supervisor()?;
    blocking(move || {
        sup.remove(&service_id)
            .map(|kept_data_path| ServiceRemoved { kept_data_path })
    })
    .await
}

/// Opens a service's own web interface (e.g. Mailpit) in the default browser. The URL is
/// built from the service's stored loopback port, never from caller input.
#[tauri::command]
pub async fn open_service_ui(state: State<'_, AppState>, service_id: String) -> IpcResult<String> {
    let db = state.db()?;
    blocking(move || {
        let svc = db.read(|c| services::get(c, &service_id))?;
        let port = svc.admin_port.ok_or_else(|| {
            crate::error::IpcError::new(
                crate::error::ErrorCode::ValidationError,
                "This service has no web interface.",
                false,
            )
        })?;
        let url = format!("http://127.0.0.1:{port}/");
        crate::platform::windows::process::open_url(&url).map_err(|_| {
            crate::error::IpcError::new(
                crate::error::ErrorCode::Internal,
                "Could not open the browser.",
                true,
            )
        })?;
        Ok(url)
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortStatus {
    pub port: u16,
    /// Program listening on the port, if any: "httpd.exe (PID 1234)".
    pub used_by: Option<String>,
    /// Devluma service the port is already assigned to, if any.
    pub assigned_to: Option<String>,
}

/// Whether a port is free for a new service (for the Add service form).
#[tauri::command]
pub async fn check_port(state: State<'_, AppState>, port: u16) -> IpcResult<PortStatus> {
    let db = state.db()?;
    blocking(move || {
        let assigned_to = db.read(services::list)?.into_iter().find_map(|s| {
            (s.port == Some(i64::from(port)) || s.admin_port == Some(i64::from(port)))
                .then_some(s.instance_name)
        });
        Ok(PortStatus {
            port,
            used_by: ports::used_by(port),
            assigned_to,
        })
    })
    .await
}

/// Changes a stopped service's ports.
#[tauri::command]
pub async fn set_service_ports(
    state: State<'_, AppState>,
    service_id: String,
    port: u16,
    admin_port: Option<u16>,
) -> IpcResult<ServiceInstance> {
    let sup = state.supervisor()?;
    blocking(move || sup.set_ports(&service_id, port, admin_port)).await
}
