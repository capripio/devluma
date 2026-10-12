use tauri::State;

use super::blocking;
use crate::error::{IpcError, IpcResult};
use crate::persistence::Database;
use crate::persistence::certificates::{self as repo, Certificate};
use crate::persistence::operations::NewOperation;
use crate::persistence::sites;
use crate::services::certificates::CertificateView;
use crate::services::journal::{Journal, ProgressSink};
use crate::services::supervisor::behavior::Failure;
use crate::state::AppState;

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

/// Runs `work` as a short journaled operation so security-relevant actions are auditable.
fn journaled<T>(
    db: &Database,
    sink: &dyn ProgressSink,
    op_type: &str,
    target: Option<&str>,
    work: impl FnOnce() -> Result<T, Failure>,
) -> IpcResult<T> {
    let journal = Journal::new(db, sink);
    let op = journal.start(&NewOperation {
        operation_type: op_type,
        target_type: Some("certificate"),
        target_id: target,
    })?;
    let _ = journal.running(&op.id);
    match work() {
        Ok(v) => {
            let _ = journal.succeed(&op.id);
            Ok(v)
        }
        Err(f) => {
            let _ = journal.fail(&op.id, f.code, &f.message);
            Err(ipc(f))
        }
    }
}

#[tauri::command]
pub async fn list_certificates(state: State<'_, AppState>) -> IpcResult<Vec<CertificateView>> {
    let certs = state.certificates()?;
    blocking(move || certs.list().map_err(ipc)).await
}

/// Creates the local CA (if missing). Doesn't trust it.
#[tauri::command]
pub async fn create_local_ca(state: State<'_, AppState>) -> IpcResult<Certificate> {
    let (certs, db, sink) = (state.certificates()?, state.db()?, state.sink());
    blocking(move || {
        journaled(&db, sink.as_ref(), "create_local_ca", None, || {
            certs.ensure_ca()
        })
    })
    .await
}

/// Generates a certificate for local domains, signed by the local CA. Doesn't trust anything.
#[tauri::command]
pub async fn create_dev_certificate(
    state: State<'_, AppState>,
    domains: Vec<String>,
) -> IpcResult<Certificate> {
    let (certs, db, sink) = (state.certificates()?, state.db()?, state.sink());
    blocking(move || {
        journaled(&db, sink.as_ref(), "create_certificate", None, || {
            certs.issue(&domains)
        })
    })
    .await
}

/// Adds the local CA to the current user's trusted roots. Windows shows its own dialog.
#[tauri::command]
pub async fn trust_certificate(state: State<'_, AppState>) -> IpcResult<Certificate> {
    let (certs, db, sink) = (state.certificates()?, state.db()?, state.sink());
    blocking(move || {
        journaled(&db, sink.as_ref(), "trust_local_ca", None, || {
            certs.trust_ca()
        })
    })
    .await
}

/// Removes the local CA from the trusted roots. Keys and files are kept.
#[tauri::command]
pub async fn remove_certificate_trust(state: State<'_, AppState>) -> IpcResult<Certificate> {
    let (certs, db, sink) = (state.certificates()?, state.db()?, state.sink());
    blocking(move || {
        journaled(&db, sink.as_ref(), "untrust_local_ca", None, || {
            certs.untrust_ca()
        })
    })
    .await
}

/// Re-issues a site certificate in place and reloads web servers that use it.
#[tauri::command]
pub async fn renew_certificate(
    state: State<'_, AppState>,
    certificate_id: String,
) -> IpcResult<Certificate> {
    let (certs, db, sink, sup) = (
        state.certificates()?,
        state.db()?,
        state.sink(),
        state.supervisor()?,
    );
    blocking(move || {
        let target = certificate_id.clone();
        journaled(
            &db,
            sink.as_ref(),
            "renew_certificate",
            Some(&target),
            || {
                let renewed = certs.renew(&certificate_id)?;
                let site_ids = db
                    .read(|c| repo::bound_sites(c, &certificate_id))
                    .map_err(|e| {
                        crate::services::supervisor::behavior::failure(
                            crate::error::ErrorCode::Internal,
                            e.to_string(),
                        )
                    })?;
                let mut servers: Vec<String> = site_ids
                    .iter()
                    .filter_map(|id| {
                        db.read(|c| sites::get(c, id))
                            .ok()
                            .map(|s| s.web_server_service_id)
                    })
                    .collect();
                servers.sort();
                servers.dedup();
                for server in servers {
                    sup.reload_if_running(&server)?;
                }
                Ok(renewed)
            },
        )
    })
    .await
}

/// Deletes a certificate and its key (refused while in use or trusted).
#[tauri::command]
pub async fn delete_certificate(
    state: State<'_, AppState>,
    certificate_id: String,
) -> IpcResult<()> {
    let (certs, db, sink) = (state.certificates()?, state.db()?, state.sink());
    blocking(move || {
        let target = certificate_id.clone();
        journaled(
            &db,
            sink.as_ref(),
            "delete_certificate",
            Some(&target),
            || certs.delete(&certificate_id),
        )
    })
    .await
}
