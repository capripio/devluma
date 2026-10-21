//! Service supervisor (trd.md §7, ADR 0006).
//!
//! - Start: port check (report the owner, never kill it) → write managed config → the
//!   service's own validator → spawn inside the service's job object → wait until it accepts
//!   connections or exits. Output goes to `services/<id>/logs/service.log`.
//! - Stop: the adapter's graceful shutdown, then — only if needed — terminate the service's
//!   job (its owned process tree). Never by process name.
//! - An exit nobody asked for marks the service `failed` with its exit code.
//! - After a Devluma restart, a still-running service is re-adopted only if PID, creation
//!   time and executable path all match what was recorded *and* it's in the service's job.
//!   Otherwise it's left alone and the service is shown as stopped.
//! - Start/stop/restart/create run in the background as journaled operations.

mod backends;
pub mod behavior;

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use self::behavior::{Failure, ServiceBehavior, ServiceCtx, failure};
use super::journal::{Journal, ProgressSink};
use crate::domain::adapter::{AdapterId, ServiceRole};
use crate::domain::catalog::Catalog;
use crate::domain::service::ServiceState;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::{EventLevel, NewOperation};
use crate::persistence::services::{self, NewService, ServiceInstance};
use crate::persistence::{Database, DbError};
use crate::platform::windows::{job, ports};
use crate::security::{paths as pathpolicy, redact};
use crate::state::DataPaths;

pub const STATE_EVENT: &str = "service://state";
const STOP_GRACE: Duration = Duration::from_secs(20);
const MAX_CONSOLE_LOG: u64 = 5 * 1024 * 1024;

/// `service://state` payload.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub service_id: String,
    pub observed_state: ServiceState,
    pub health_status: String,
    pub pid: Option<i64>,
    pub last_exit_code: Option<i64>,
    pub last_error_code: Option<String>,
}

impl From<&ServiceInstance> for ServiceStatus {
    fn from(s: &ServiceInstance) -> Self {
        Self {
            service_id: s.id.clone(),
            observed_state: s.observed_state,
            health_status: s.health_status.clone(),
            pid: s.pid,
            last_exit_code: s.last_exit_code,
            last_error_code: s.last_error_code.clone(),
        }
    }
}

pub trait StateSink: Send + Sync {
    fn emit_state(&self, status: &ServiceStatus);
}

impl<R: tauri::Runtime> StateSink for tauri::AppHandle<R> {
    fn emit_state(&self, status: &ServiceStatus) {
        use tauri::Emitter;
        let _ = Emitter::emit(self, STATE_EVENT, status);
    }
}

/// A service process Devluma currently owns.
struct Live {
    job: Arc<job::Job>,
}

pub struct Supervisor {
    db: Arc<Database>,
    catalog: Arc<Catalog>,
    paths: DataPaths,
    progress: Arc<dyn ProgressSink>,
    states: Arc<dyn StateSink>,
    busy: Mutex<HashSet<String>>,
    live: Mutex<HashMap<String, Live>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateServiceRequest {
    pub installation_id: String,
    pub name: String,
    pub port: u16,
    /// Required for services with a UI port (see `ServiceBehavior::admin_port`).
    #[serde(default)]
    pub admin_port: Option<u16>,
    /// Databases only. Defaults to `<root>/data/<package>/<name>`.
    #[serde(default)]
    pub data_path: Option<String>,
    /// Databases only: adopt an already-initialized data folder instead of creating one.
    #[serde(default)]
    pub use_existing_data: bool,
}

pub type Started = (String, tauri::async_runtime::JoinHandle<()>);

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

fn db_fail(e: DbError) -> Failure {
    let i = IpcError::from(e);
    failure(i.code, i.message)
}

impl Supervisor {
    pub fn new(
        db: Arc<Database>,
        catalog: Arc<Catalog>,
        paths: DataPaths,
        progress: Arc<dyn ProgressSink>,
        states: Arc<dyn StateSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            catalog,
            paths,
            progress,
            states,
            busy: Mutex::new(HashSet::new()),
            live: Mutex::new(HashMap::new()),
        })
    }

    fn emit(&self, id: &str) {
        if let Ok(s) = self.db.read(|c| services::get(c, id)) {
            self.states.emit_state(&ServiceStatus::from(&s));
        }
    }

    fn get(&self, id: &str) -> Result<ServiceInstance, Failure> {
        self.db.read(|c| services::get(c, id)).map_err(db_fail)
    }

    /// Resolves the behaviour and paths for a service.
    fn context(
        &self,
        svc: &ServiceInstance,
    ) -> Result<(ServiceCtx, &'static dyn ServiceBehavior), Failure> {
        let inst = self
            .db
            .read(|c| installations::get(c, &svc.package_installation_id))
            .map_err(db_fail)?;
        if inst.status != InstallStatus::Installed {
            return Err(failure(
                ErrorCode::ValidationError,
                "The installed version this service uses is missing or broken. Reinstall it first.",
            ));
        }
        let behavior = AdapterId::parse(&svc.lifecycle_adapter)
            .and_then(behavior::for_adapter)
            .ok_or_else(|| {
                failure(
                    ErrorCode::ValidationError,
                    "This service type isn't supported by this build.",
                )
            })?;
        let port = svc
            .port
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| {
                failure(
                    ErrorCode::ValidationError,
                    "This service has no valid port.",
                )
            })?;
        Ok((
            ServiceCtx {
                service_id: svc.id.clone(),
                install_dir: PathBuf::from(&inst.install_path),
                service_dir: self.paths.services.join(&svc.id),
                data_dir: svc.data_path.as_ref().map(PathBuf::from),
                port,
                admin_port: svc.admin_port.and_then(|p| u16::try_from(p).ok()),
            },
            behavior,
        ))
    }

    fn claim(self: &Arc<Self>, id: &str) -> Result<BusyGuard, IpcError> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if !busy.insert(id.to_owned()) {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "Another action is already running for this service.",
                false,
            ));
        }
        Ok(BusyGuard {
            sup: Arc::clone(self),
            id: id.to_owned(),
        })
    }

    /// Runs `work` as a journaled background operation for one service.
    fn run_op(
        self: &Arc<Self>,
        id: &str,
        op_type: &'static str,
        work: impl FnOnce(&Arc<Self>, &str) -> Result<(), Failure> + Send + 'static,
    ) -> Result<Started, IpcError> {
        // Claimed now (so a second click fails fast) and released when the task finishes.
        let guard = self.claim(id)?;
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let target = format!("service:{id}");
        let op = journal
            .start(&NewOperation {
                operation_type: op_type,
                target_type: Some("service_instance"),
                target_id: Some(&target),
            })
            .map_err(IpcError::from)?;
        let (me, op_id) = (Arc::clone(self), op.id.clone());
        let handle = tauri::async_runtime::spawn_blocking(move || {
            let _guard = guard;
            let journal = Journal::new(&me.db, me.progress.as_ref());
            let _ = journal.running(&op_id);
            let result = work(&me, &op_id);
            let _ = match result {
                Ok(()) => journal.succeed(&op_id).map(|_| ()),
                Err(f) => journal.fail(&op_id, f.code, &f.message).map(|_| ()),
            };
        });
        Ok((op.id, handle))
    }

    fn journal(&self) -> Journal<'_> {
        Journal::new(&self.db, self.progress.as_ref())
    }

    // ---- start ------------------------------------------------------------------------

    pub fn start(self: &Arc<Self>, id: &str) -> Result<Started, IpcError> {
        let svc = self.get(id).map_err(ipc)?;
        if !svc.observed_state.can_start() {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                format!(
                    "The service is {}; it can't be started now.",
                    svc.observed_state.as_str()
                ),
                false,
            ));
        }
        let sid = id.to_owned();
        self.run_op(id, "start_service", move |me, op| me.do_start(op, &sid))
    }

    fn do_start(self: &Arc<Self>, op: &str, id: &str) -> Result<(), Failure> {
        let svc = self.get(id)?;
        if !svc.observed_state.can_start() {
            return Err(failure(
                ErrorCode::ValidationError,
                "The service is no longer stopped.",
            ));
        }
        let (ctx, beh) = self.context(&svc)?;
        let journal = self.journal();
        let record_error = |f: Failure| -> Failure {
            let _ = self
                .db
                .write(|c| services::set_outcome(c, id, None, Some(f.code.as_str())));
            self.emit(id);
            f
        };

        let _ = journal.progress(op, "Checking ports", None, None);
        for port in std::iter::once(ctx.port).chain(ctx.admin_port) {
            let Some(who) = ports::used_by(port) else {
                continue;
            };
            return Err(record_error(failure(
                ErrorCode::PortInUse,
                format!(
                    "Port {port} is already used by {who}. Choose another port for this service or stop that program yourself; Devluma won't touch it."
                ),
            )));
        }

        let _ = journal.progress(op, "Preparing configuration", None, None);
        beh.prepare(&ctx).map_err(record_error)?;
        beh.validate(&ctx).map_err(record_error)?;

        self.db
            .write(|c| {
                services::set_desired(c, id, "running")?;
                services::set_outcome(c, id, None, None)?;
                services::transition(c, id, ServiceState::Starting).map(|_| ())
            })
            .map_err(db_fail)?;
        self.emit(id);
        let _ = journal.progress(op, "Starting", None, None);

        match self.spawn_and_wait_ready(id, &ctx, beh) {
            Ok(()) => {
                self.db
                    .write(|c| {
                        services::set_health(c, id, "healthy")?;
                        services::transition(c, id, ServiceState::Running).map(|_| ())
                    })
                    .map_err(db_fail)?;
                self.emit(id);
                Ok(())
            }
            Err((f, exit_code)) => {
                self.kill_owned(id);
                let _ = self.db.write(|c| {
                    services::set_process(c, id, None, None, None)?;
                    services::set_outcome(c, id, exit_code.map(i64::from), Some(f.code.as_str()))?;
                    services::set_health(c, id, "unknown")?;
                    services::transition(c, id, ServiceState::Failed).map(|_| ())
                });
                self.emit(id);
                Err(f)
            }
        }
    }

    fn spawn_and_wait_ready(
        self: &Arc<Self>,
        id: &str,
        ctx: &ServiceCtx,
        beh: &dyn ServiceBehavior,
    ) -> Result<(), (Failure, Option<i32>)> {
        let plain = |f: Failure| (f, None);
        std::fs::create_dir_all(ctx.log_dir()).map_err(|e| {
            plain(failure(
                ErrorCode::Internal,
                format!("Could not create the log folder: {e}"),
            ))
        })?;
        rotate(&ctx.console_log());
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(ctx.console_log())
            .map_err(|e| {
                plain(failure(
                    ErrorCode::Internal,
                    format!("Could not open the service log: {e}"),
                ))
            })?;
        let log_err = log
            .try_clone()
            .map_err(|e| plain(failure(ErrorCode::Internal, e.to_string())))?;

        // A fresh job per run: a terminated job can't accept new processes.
        let job_name = job::new_job_name(id);
        let job = Arc::new(job::Job::create(&job_name).map_err(|e| {
            plain(failure(
                ErrorCode::Internal,
                format!("Could not create a job object: {e}"),
            ))
        })?);

        let launch = beh.launch(ctx);
        let mut cmd = Command::new(&launch.exe);
        cmd.args(&launch.args)
            .current_dir(&launch.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .env_clear();
        for key in [
            "SystemRoot",
            "SystemDrive",
            "WINDIR",
            "TEMP",
            "TMP",
            "COMPUTERNAME",
            "USERNAME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "ProgramData",
            "PATHEXT",
        ] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        if let Some(root) = std::env::var_os("SystemRoot") {
            let root = PathBuf::from(root);
            cmd.env(
                "PATH",
                format!("{};{}", root.join("System32").display(), root.display()),
            );
        }
        for (k, v) in &launch.env {
            cmd.env(k, v);
        }

        let mut child = job.spawn(&mut cmd).map_err(|e| {
            plain(failure(
                ErrorCode::ProcessExited,
                format!("Could not start {}: {e}", launch.exe.display()),
            ))
        })?;
        let pid = child.id();
        let created = job::identity(pid).map(|i| i.created.to_string());
        let _ = self.db.write(|c| {
            services::set_process(c, id, Some(pid), created.as_deref(), Some(&job_name))
        });

        let exited = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(None));
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(
            id.to_owned(),
            Live {
                job: Arc::clone(&job),
            },
        );
        {
            let (me, sid, exited, exit_code) = (
                Arc::clone(self),
                id.to_owned(),
                Arc::clone(&exited),
                Arc::clone(&exit_code),
            );
            std::thread::spawn(move || {
                let code = child.wait().ok().and_then(|s| s.code());
                *exit_code.lock().unwrap_or_else(|e| e.into_inner()) = code;
                exited.store(true, Ordering::SeqCst);
                me.on_exit(&sid, code);
            });
        }

        let deadline = Instant::now() + beh.ready_timeout();
        loop {
            if exited.load(Ordering::SeqCst) {
                let code = *exit_code.lock().unwrap_or_else(|e| e.into_inner());
                return Err((
                    failure(
                        ErrorCode::ProcessExited,
                        format!(
                            "The service exited during startup (exit code {}). {}",
                            code.map_or("unknown".into(), |c| c.to_string()),
                            last_lines_hint(&ctx.console_log())
                        ),
                    ),
                    code,
                ));
            }
            if ports::accepts_connections(ctx.port, Duration::from_millis(300)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(plain(failure(
                    ErrorCode::ProcessExited,
                    format!(
                        "The service didn't start accepting connections on port {} within {} seconds, so it was stopped. {}",
                        ctx.port,
                        beh.ready_timeout().as_secs(),
                        last_lines_hint(&ctx.console_log())
                    ),
                )));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Called from the monitor thread when the service's main process exits.
    fn on_exit(&self, id: &str, code: Option<i32>) {
        let Ok(svc) = self.get(id) else { return };
        let unexpected = svc.desired_state == "running"
            && matches!(
                svc.observed_state,
                ServiceState::Running | ServiceState::Degraded
            );
        if !unexpected {
            return; // start/stop flows own these transitions
        }
        self.kill_owned(id);
        let error = ErrorCode::ProcessExited;
        let _ = self.db.write(|c| {
            services::set_process(c, id, None, None, None)?;
            services::set_outcome(c, id, code.map(i64::from), Some(error.as_str()))?;
            services::set_health(c, id, "unknown")?;
            services::transition(c, id, ServiceState::Failed).map(|_| ())
        });
        self.emit(id);
        // Journal it so the failure shows up in the operations tray and logs.
        let journal = self.journal();
        let target = format!("service:{id}");
        if let Ok(op) = journal.start(&NewOperation {
            operation_type: "service_exited",
            target_type: Some("service_instance"),
            target_id: Some(&target),
        }) {
            let _ = journal.running(&op.id);
            let _ = journal.fail(
                &op.id,
                error,
                &format!(
                    "{} stopped unexpectedly (exit code {}).",
                    svc.instance_name,
                    code.map_or("unknown".into(), |c| c.to_string())
                ),
            );
        }
    }

    /// Terminates whatever is left in this service's job, if anything.
    fn kill_owned(&self, id: &str) {
        let job = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .map(|l| l.job)
            .or_else(|| {
                self.get(id)
                    .ok()
                    .and_then(|s| s.job_name)
                    .and_then(|n| job::Job::open(&n))
                    .map(Arc::new)
            });
        if let Some(job) = job
            && job.process_ids().map(|p| !p.is_empty()).unwrap_or(false)
        {
            let _ = job.terminate(1);
            job.wait_empty(Duration::from_secs(10));
        }
    }

    // ---- stop / restart ---------------------------------------------------------------

    pub fn stop(self: &Arc<Self>, id: &str) -> Result<Started, IpcError> {
        let svc = self.get(id).map_err(ipc)?;
        if !svc.observed_state.can_stop() {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                format!(
                    "The service is {}; there's nothing to stop.",
                    svc.observed_state.as_str()
                ),
                false,
            ));
        }
        let sid = id.to_owned();
        self.run_op(id, "stop_service", move |me, op| me.do_stop(op, &sid))
    }

    pub fn restart(self: &Arc<Self>, id: &str) -> Result<Started, IpcError> {
        let svc = self.get(id).map_err(ipc)?;
        if !svc.observed_state.can_stop() {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "Only a running service can be restarted.",
                false,
            ));
        }
        let sid = id.to_owned();
        self.run_op(id, "restart_service", move |me, op| {
            me.do_stop(op, &sid)?;
            me.do_start(op, &sid)
        })
    }

    fn do_stop(&self, op: &str, id: &str) -> Result<(), Failure> {
        let svc = self.get(id)?;
        if !svc.observed_state.can_stop() {
            return Ok(());
        }
        let journal = self.journal();
        self.db
            .write(|c| {
                services::set_desired(c, id, "stopped")?;
                services::transition(c, id, ServiceState::Stopping).map(|_| ())
            })
            .map_err(db_fail)?;
        self.emit(id);
        let _ = journal.progress(op, "Stopping", None, None);

        let ctx = self.context(&svc).ok();
        let job = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .map(|l| Arc::clone(&l.job))
            .or_else(|| {
                svc.job_name
                    .as_deref()
                    .and_then(job::Job::open)
                    .map(Arc::new)
            });

        let pid = svc.pid.and_then(|p| u32::try_from(p).ok());
        let (mut requested, mut graceful) = (false, false);
        if let (Some(pid), Some((ctx, beh))) = (pid, ctx.as_ref())
            && beh.request_stop(ctx, pid)
        {
            requested = true;
            graceful = job::wait_for_exit(pid, Some(STOP_GRACE))
                && job
                    .as_ref()
                    .is_none_or(|j| j.wait_empty(Duration::from_secs(5)));
        }
        if !graceful
            && let Some(job) = &job
            && job.process_ids().map(|p| !p.is_empty()).unwrap_or(false)
        {
            if requested {
                let _ = journal.event(
                    op,
                    EventLevel::Warn,
                    "Graceful shutdown didn't finish in time; stopping the service's own processes.",
                    &serde_json::json!({}),
                );
            }
            let _ = job.terminate(1);
            if !job.wait_empty(Duration::from_secs(10)) {
                let _ = self
                    .db
                    .write(|c| services::transition(c, id, ServiceState::Failed).map(|_| ()));
                self.emit(id);
                return Err(failure(
                    ErrorCode::ProcessExited,
                    "Some of the service's processes could not be stopped.",
                ));
            }
        }
        // Windows releases a terminated process's sockets slightly after it leaves the job;
        // "stopped" should mean the ports can be used again.
        if let (Some(pid), Some((ctx, _))) = (pid, ctx.as_ref()) {
            let svc_ports: Vec<u16> = std::iter::once(ctx.port).chain(ctx.admin_port).collect();
            wait_ports_released(pid, &svc_ports, Duration::from_secs(5));
        }
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.db
            .write(|c| {
                services::set_process(c, id, None, None, None)?;
                services::set_health(c, id, "unknown")?;
                services::transition(c, id, ServiceState::Stopped).map(|_| ())
            })
            .map_err(db_fail)?;
        self.emit(id);
        Ok(())
    }

    /// Stops every live service (app exit). Blocks until done or `timeout` per service.
    pub fn stop_all_blocking(self: &Arc<Self>) {
        let live: Vec<String> = self
            .db
            .read(services::list)
            .map(|l| {
                l.into_iter()
                    .filter(|s| s.observed_state.can_stop())
                    .map(|s| s.id)
                    .collect()
            })
            .unwrap_or_default();
        std::thread::scope(|scope| {
            for id in &live {
                scope.spawn(move || {
                    let journal = self.journal();
                    let target = format!("service:{id}");
                    if let Ok(op) = journal.start(&NewOperation {
                        operation_type: "stop_service",
                        target_type: Some("service_instance"),
                        target_id: Some(&target),
                    }) {
                        let _ = journal.running(&op.id);
                        let _ = match self.do_stop(&op.id, id) {
                            Ok(()) => journal.succeed(&op.id).map(|_| ()),
                            Err(f) => journal.fail(&op.id, f.code, &f.message).map(|_| ()),
                        };
                    }
                });
            }
        });
    }

    // ---- adoption & health ------------------------------------------------------------

    /// Startup: services left `unknown` by recovery are adopted if provably ours, else stopped.
    pub fn reconcile(self: &Arc<Self>) {
        let Ok(all) = self.db.read(services::list) else {
            return;
        };
        for svc in all
            .into_iter()
            .filter(|s| s.observed_state == ServiceState::Unknown)
        {
            if self.try_adopt(&svc) {
                continue;
            }
            let _ = self.db.write(|c| {
                services::set_process(c, &svc.id, None, None, None)?;
                services::set_health(c, &svc.id, "unknown")?;
                services::transition(c, &svc.id, ServiceState::Stopped).map(|_| ())
            });
            self.emit(&svc.id);
        }
    }

    fn try_adopt(self: &Arc<Self>, svc: &ServiceInstance) -> bool {
        let (Some(pid), Some(created)) = (
            svc.pid.and_then(|p| u32::try_from(p).ok()),
            svc.process_started_at.as_deref(),
        ) else {
            return false;
        };
        let Ok((ctx, beh)) = self.context(svc) else {
            return false;
        };
        let Some(ident) = job::identity(pid) else {
            return false;
        };
        let Some(job) = svc.job_name.as_deref().and_then(job::Job::open) else {
            return false;
        };
        let owned = ident.created.to_string() == created
            && job::same_image(&ident.image, &beh.image(&ctx))
            && job
                .process_ids()
                .map(|ids| ids.contains(&pid))
                .unwrap_or(false);
        if !owned {
            return false;
        }
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(svc.id.clone(), Live { job: Arc::new(job) });
        let healthy = ports::accepts_connections(ctx.port, Duration::from_secs(1));
        let _ = self.db.write(|c| {
            services::set_desired(c, &svc.id, "running")?;
            services::set_health(c, &svc.id, if healthy { "healthy" } else { "unreachable" })?;
            services::transition(
                c,
                &svc.id,
                if healthy {
                    ServiceState::Running
                } else {
                    ServiceState::Degraded
                },
            )
            .map(|_| ())
        });
        self.emit(&svc.id);
        let (me, sid) = (Arc::clone(self), svc.id.clone());
        std::thread::spawn(move || {
            job::wait_for_exit(pid, None);
            me.on_exit(&sid, None);
        });
        true
    }

    /// One round of health checks for running services (called periodically).
    pub fn health_tick(&self) {
        let Ok(all) = self.db.read(services::list) else {
            return;
        };
        for svc in all.into_iter().filter(|s| {
            matches!(
                s.observed_state,
                ServiceState::Running | ServiceState::Degraded
            )
        }) {
            let Some(port) = svc.port.and_then(|p| u16::try_from(p).ok()) else {
                continue;
            };
            let ok = ports::accepts_connections(port, Duration::from_secs(1));
            let (to, health) = if ok {
                (ServiceState::Running, "healthy")
            } else {
                (ServiceState::Degraded, "unreachable")
            };
            if svc.observed_state != to || svc.health_status != health {
                let _ = self.db.write(|c| {
                    services::set_health(c, &svc.id, health)?;
                    services::transition(c, &svc.id, to).map(|_| ())
                });
                self.emit(&svc.id);
            }
        }
    }

    /// Runs health checks every `every` on a background thread for the app's lifetime.
    pub fn spawn_health_loop(self: &Arc<Self>, every: Duration) {
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(every);
                match weak.upgrade() {
                    Some(sup) => sup.health_tick(),
                    None => break,
                }
            }
        });
    }

    // ---- create / remove / details ----------------------------------------------------

    pub fn create(self: &Arc<Self>, req: CreateServiceRequest) -> Result<Started, IpcError> {
        let invalid = |m: String| IpcError::new(ErrorCode::ValidationError, m, false);
        let inst = self
            .db
            .read(|c| installations::get(c, &req.installation_id))?;
        if inst.status != InstallStatus::Installed {
            return Err(invalid(
                "Install this version before creating a service from it.".into(),
            ));
        }
        let pkg = self
            .catalog
            .package(&inst.package_id)
            .ok_or_else(|| invalid("This package is no longer in the catalog.".into()))?;
        let role = pkg.adapter.service_role().ok_or_else(|| {
            invalid(format!(
                "{} doesn't run as a service.",
                pkg.manifest.display_name
            ))
        })?;
        let name = req.name.trim().to_owned();
        if name.is_empty()
            || name.len() > 60
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(invalid(
                "Use 1–60 letters, digits, '-' or '_' for the service name.".into(),
            ));
        }
        let beh = behavior::for_adapter(pkg.adapter)
            .ok_or_else(|| invalid("This service type isn't supported by this build.".into()))?;
        let admin_port = match (beh.admin_port(), req.admin_port) {
            (Some(a), None) => {
                return Err(invalid(format!("Choose a port for the {}.", a.label)));
            }
            (Some(_), Some(p)) => Some(p),
            (None, _) => None,
        };
        if req.port == 0 || admin_port == Some(0) {
            return Err(invalid("Choose a port between 1 and 65535.".into()));
        }
        if admin_port == Some(req.port) {
            return Err(invalid("Use two different ports.".into()));
        }
        let existing = self.db.read(services::list)?;
        for port in std::iter::once(req.port).chain(admin_port) {
            if let Some(other) = existing
                .iter()
                .find(|s| s.port == Some(i64::from(port)) || s.admin_port == Some(i64::from(port)))
            {
                return Err(invalid(format!(
                    "Port {port} is already assigned to the service \"{}\".",
                    other.instance_name
                )));
            }
        }

        let data_dir = match role {
            ServiceRole::Database => Some(match &req.data_path {
                Some(p) if !p.trim().is_empty() => {
                    let p = PathBuf::from(p.trim());
                    if !p.is_absolute() {
                        return Err(invalid("Enter the full path of the data folder.".into()));
                    }
                    p
                }
                _ => self.paths.data.join(&inst.package_id).join(&name),
            }),
            ServiceRole::WebServer | ServiceRole::FastCgi | ServiceRole::Mail => None,
        };
        if let Some(dir) = &data_dir {
            let initialized = beh.is_data_initialized(dir);
            if req.use_existing_data && !initialized {
                return Err(invalid(format!(
                    "{} doesn't contain an initialized database.",
                    dir.display()
                )));
            }
            let non_empty = dir.exists()
                && std::fs::read_dir(dir)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(true);
            if !req.use_existing_data && non_empty {
                return Err(IpcError::new(
                    ErrorCode::DataDirectoryExists,
                    format!(
                        "{} already contains files. Choose an empty folder, or select \"Use the existing data in this folder\" if it's a {} data folder.",
                        dir.display(),
                        pkg.manifest.display_name
                    ),
                    false,
                ));
            }
        }

        let journal = self.journal();
        let op = journal.start(&NewOperation {
            operation_type: "create_service",
            target_type: Some("package_installation"),
            target_id: Some(&inst.id),
        })?;
        let me = Arc::clone(self);
        let (op_id, adapter) = (op.id.clone(), pkg.adapter);
        let install_dir = PathBuf::from(&inst.install_path);
        let handle = tauri::async_runtime::spawn_blocking(move || {
            let journal = me.journal();
            let _ = journal.running(&op_id);
            let result = (|| -> Result<(), Failure> {
                if let (Some(dir), false) = (&data_dir, req.use_existing_data) {
                    let _ = journal.progress(&op_id, "Initializing the data folder", None, None);
                    beh.initialize_data(&install_dir, dir, req.port)?;
                }
                let data = data_dir.as_ref().map(|d| d.display().to_string());
                let svc = me
                    .db
                    .write(|c| {
                        services::create(
                            c,
                            &NewService {
                                package_installation_id: &req.installation_id,
                                instance_name: &name,
                                service_type: match role {
                                    ServiceRole::WebServer => "web_server",
                                    ServiceRole::Database => "database",
                                    ServiceRole::FastCgi => "fastcgi",
                                    ServiceRole::Mail => "mail",
                                },
                                lifecycle_adapter: adapter.as_str(),
                                port: Some(req.port),
                                admin_port,
                                bind_address: Some("127.0.0.1"),
                                config_path: None,
                                data_path: data.as_deref(),
                            },
                        )
                    })
                    .map_err(db_fail)?;
                let (ctx, beh) = me.context(&svc)?;
                beh.prepare(&ctx)?;
                let config = beh.config_path(&ctx).display().to_string();
                me.db
                    .write(|c| services::set_paths(c, &svc.id, Some(&config), data.as_deref()))
                    .map_err(db_fail)?;
                me.emit(&svc.id);
                Ok(())
            })();
            let _ = match result {
                Ok(()) => journal.succeed(&op_id).map(|_| ()),
                Err(f) => journal.fail(&op_id, f.code, &f.message).map(|_| ()),
            };
        });
        Ok((op.id, handle))
    }

    /// Changes the ports of a stopped service. Web servers with sites keep their port (the
    /// sites' configs and URLs use it); remove or re-create those sites instead.
    pub fn set_ports(
        self: &Arc<Self>,
        id: &str,
        port: u16,
        admin_port: Option<u16>,
    ) -> Result<ServiceInstance, IpcError> {
        let invalid = |m: String| IpcError::new(ErrorCode::ValidationError, m, false);
        let _guard = self.claim(id)?;
        let svc = self.get(id).map_err(ipc)?;
        if svc.observed_state.is_live() {
            return Err(invalid(
                "Stop the service before changing its ports.".into(),
            ));
        }
        let (_, beh) = self.context(&svc).map_err(ipc)?;
        let admin_port = match (beh.admin_port(), admin_port) {
            (Some(a), None) => return Err(invalid(format!("Choose a port for the {}.", a.label))),
            (Some(_), p) => p,
            (None, _) => None,
        };
        if port == 0 || admin_port == Some(0) || admin_port == Some(port) {
            return Err(invalid(
                "Choose two different ports between 1 and 65535.".into(),
            ));
        }
        if svc.service_type == "web_server"
            && svc.port != Some(i64::from(port))
            && self
                .db
                .read(crate::persistence::sites::list)?
                .iter()
                .any(|s| s.web_server_service_id == id)
        {
            return Err(invalid(
                "Sites use this web server's port. Remove those sites (or create another web server) to use a different port.".into(),
            ));
        }
        for p in std::iter::once(port).chain(admin_port) {
            if let Some(other) = self.db.read(services::list)?.iter().find(|s| {
                s.id != id && (s.port == Some(i64::from(p)) || s.admin_port == Some(i64::from(p)))
            }) {
                return Err(invalid(format!(
                    "Port {p} is already assigned to the service \"{}\".",
                    other.instance_name
                )));
            }
        }
        self.db
            .write(|c| services::set_ports(c, id, port, admin_port))?;
        self.emit(id);
        self.get(id).map_err(ipc)
    }

    /// Removes a stopped service: its record and Devluma-managed folder. Data folders are kept.
    pub fn remove(self: &Arc<Self>, id: &str) -> Result<Option<String>, IpcError> {
        let _guard = self.claim(id)?;
        let svc = self.get(id).map_err(ipc)?;
        if svc.observed_state.is_live() {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "Stop the service before removing it.",
                false,
            ));
        }
        self.db.write(|c| services::delete(c, id))?;
        let dir = self.paths.services.join(id);
        if dir.exists() && pathpolicy::ensure_within(&self.paths.services, &dir).is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(svc.data_path)
    }

    pub fn details(&self, id: &str) -> Result<ServiceDetails, IpcError> {
        let svc = self.get(id).map_err(ipc)?;
        let (ctx, beh) = self.context(&svc).map_err(ipc)?;
        let config_path = beh.config_path(&ctx);
        let config_text = std::fs::read_to_string(&config_path)
            .ok()
            .map(|t| redact::redact(&t.chars().take(64 * 1024).collect::<String>()).into_owned());
        let mut logs = vec![("service.log".to_owned(), ctx.console_log())];
        logs.extend(beh.log_files(&ctx));
        Ok(ServiceDetails {
            service_dir: ctx.service_dir.display().to_string(),
            config_path: config_path.display().to_string(),
            config_text,
            data_path: svc.data_path.clone(),
            logs: logs
                .into_iter()
                .map(|(label, path)| LogTail {
                    lines: tail(&path, 200),
                    label,
                    path: path.display().to_string(),
                })
                .collect(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDetails {
    pub service_dir: String,
    pub config_path: String,
    pub config_text: Option<String>,
    pub data_path: Option<String>,
    pub logs: Vec<LogTail>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogTail {
    pub label: String,
    pub path: String,
    /// Last lines, redacted.
    pub lines: Vec<String>,
}

struct BusyGuard {
    sup: Arc<Supervisor>,
    id: String,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.sup
            .busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

/// Keeps the console log bounded: one previous generation.
fn rotate(path: &Path) {
    if std::fs::metadata(path)
        .map(|m| m.len() > MAX_CONSOLE_LOG)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
}

fn tail(path: &Path, n: usize) -> Vec<String> {
    let Ok(text) = std::fs::read(path) else {
        return Vec::new();
    };
    let start = text.len().saturating_sub(256 * 1024);
    String::from_utf8_lossy(&text[start..])
        .lines()
        .rev()
        .take(n)
        .map(|l| redact::redact(l).into_owned())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn last_lines_hint(path: &Path) -> String {
    let lines = tail(path, 3);
    if lines.is_empty() {
        "See the service log for details.".into()
    } else {
        format!("Last log lines: {}", lines.join(" | "))
    }
}

/// Waits until `pid` no longer listens on any of `ports` (bounded). Another program taking a
/// port afterwards isn't our concern here; start reports that as a conflict.
fn wait_ports_released(pid: u32, ports: &[u16], timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if ports
            .iter()
            .all(|p| ports::listener_owner(*p).is_none_or(|o| o.pid != pid))
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Test helper: stops every service on drop, so a failing live test leaves nothing running.
#[cfg(test)]
pub(crate) struct StopAllOnDrop(pub Arc<Supervisor>);

#[cfg(test)]
impl Drop for StopAllOnDrop {
    fn drop(&mut self) {
        self.0.stop_all_blocking();
    }
}

#[cfg(test)]
pub(crate) mod tests;
