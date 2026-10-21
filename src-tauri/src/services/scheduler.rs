//! Native task scheduler (ADR 0017): recurring project tasks Devluma runs while it is open.
//!
//! - Jobs run in the project folder with the project's runtimes first on `PATH` (the same
//!   `PATH` the project terminal gets). The system `PATH` is never changed.
//! - Every run is an explicit executable plus an argument array (never a shell string), with a
//!   cleared environment, no console window, a timeout, and its own kill-on-close job object,
//!   so a timeout, Stop or quitting Devluma always ends the whole process tree.
//! - A job never overlaps itself: a tick while its previous run is still going is recorded as
//!   skipped. Runs missed while Devluma was closed are not caught up.
//! - Output goes to `<root>\logs\scheduler\<job>\<run>.log` and is redacted when shown.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};

use crate::domain::catalog::Catalog;
use crate::domain::schedule::Cron;
use crate::error::ErrorCode;
use crate::persistence::Database;
use crate::persistence::scheduler::{self as repo, Job, JobFields, JobKind, Run, RunStatus};
use crate::platform::windows::job;
use crate::services::supervisor::behavior::{Failure, failure};
use crate::state::DataPaths;

pub const JOB_EVENT: &str = "scheduler://job";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The most of a run's log the UI is sent (the end of it).
const LOG_TAIL_BYTES: u64 = 256 * 1024;

/// `scheduler://job` payload: something about this job changed; the UI refetches.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub job_id: String,
}

pub trait SchedulerSink: Send + Sync {
    fn job_changed(&self, job_id: &str);
}

impl<R: tauri::Runtime> SchedulerSink for tauri::AppHandle<R> {
    fn job_changed(&self, job_id: &str) {
        use tauri::Emitter;
        let _ = Emitter::emit(
            self,
            JOB_EVENT,
            JobEvent {
                job_id: job_id.to_owned(),
            },
        );
    }
}

/// A job as the add/edit form sends it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    /// Set when editing.
    pub id: Option<String>,
    pub project_id: String,
    pub name: String,
    pub kind: JobKind,
    /// `command` only: an .exe name on the project's PATH, or an absolute .exe path.
    pub program: Option<String>,
    /// `php_script`: the script (relative to the project) then its arguments; `command`: the
    /// arguments. Ignored for `artisan_schedule`.
    #[serde(default)]
    pub args: Vec<String>,
    pub cron: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn default_timeout() -> u32 {
    600
}

fn enabled_by_default() -> bool {
    true
}

/// Exactly what a job runs, shown before it is saved and on the job's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPreview {
    pub executable: String,
    pub args: Vec<String>,
    pub working_dir: String,
    /// Prepended to the system PATH for this run only.
    pub path_prefix: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobPreview {
    pub command: CommandPreview,
    pub cron: String,
    pub schedule_text: String,
    /// Local times (RFC 3339 with offset).
    pub next_runs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Active,
    Executing,
    Muted,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub name: String,
    pub kind: JobKind,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cron: String,
    pub schedule_text: String,
    pub enabled: bool,
    pub timeout_secs: u32,
    pub status: JobStatus,
    pub next_run: Option<String>,
    pub last_run: Option<Run>,
    /// Start time of the run in progress.
    pub running_since: Option<String>,
    /// What it runs, or why it can't run right now (e.g. the project's PHP was uninstalled).
    pub command: Option<CommandPreview>,
    pub problem: Option<String>,
}

/// How a run's executable is started. Tests re-run the test binary as a fake worker.
#[derive(Debug, Clone, Default)]
pub struct Launcher {
    pub prefix: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Pass the real arguments in `DEVLUMA_FAKE_WORKER_ARGS` (the test harness rejects them).
    pub args_via_env: bool,
}

struct Resolved {
    exe: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    prefix: Vec<String>,
}

impl Resolved {
    fn preview(&self) -> CommandPreview {
        CommandPreview {
            executable: self.exe.display().to_string(),
            args: self.args.clone(),
            working_dir: self.cwd.display().to_string(),
            path_prefix: self.prefix.clone(),
        }
    }
}

struct Live {
    started_at: String,
    job: Arc<job::Job>,
    stop: Arc<AtomicBool>,
}

pub struct Scheduler {
    db: Arc<Database>,
    catalog: Arc<Catalog>,
    paths: DataPaths,
    sink: Arc<dyn SchedulerSink>,
    launcher: Launcher,
    /// Tests use short timeouts; real jobs are 10 s .. 24 h.
    timeout_override: Option<Duration>,
    live: Mutex<HashMap<String, Live>>,
    last_tick: Mutex<Option<(u32, u32, u32)>>,
}

fn invalid(message: impl Into<String>) -> Failure {
    failure(ErrorCode::ValidationError, message)
}

fn internal(e: impl std::fmt::Display) -> Failure {
    failure(ErrorCode::Internal, e.to_string())
}

impl Scheduler {
    pub fn new(
        db: Arc<Database>,
        catalog: Arc<Catalog>,
        paths: DataPaths,
        sink: Arc<dyn SchedulerSink>,
    ) -> Arc<Self> {
        Self::with(db, catalog, paths, sink, Launcher::default(), None)
    }

    pub fn with(
        db: Arc<Database>,
        catalog: Arc<Catalog>,
        paths: DataPaths,
        sink: Arc<dyn SchedulerSink>,
        launcher: Launcher,
        timeout_override: Option<Duration>,
    ) -> Arc<Self> {
        // Runs still marked running belong to a previous session.
        let _ = db.write(|c| repo::close_interrupted(c));
        Arc::new(Self {
            db,
            catalog,
            paths,
            sink,
            launcher,
            timeout_override,
            live: Mutex::new(HashMap::new()),
            last_tick: Mutex::new(None),
        })
    }

    fn logs_dir(&self, job_id: &str) -> PathBuf {
        self.paths.root.join("logs").join("scheduler").join(job_id)
    }

    fn log_path(&self, job_id: &str, run_id: &str) -> PathBuf {
        self.logs_dir(job_id).join(format!("{run_id}.log"))
    }

    fn lock_live(&self) -> std::sync::MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- the clock -------------------------------------------------------------------

    /// Wakes just after each minute boundary and starts the jobs due in that minute.
    pub fn spawn_loop(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                let now = Local::now();
                let wait = 60 - u64::from(now.second());
                std::thread::sleep(Duration::from_secs(wait) + Duration::from_millis(200));
                match weak.upgrade() {
                    Some(s) => s.tick(Local::now()),
                    None => break,
                }
            }
        });
    }

    /// Starts every enabled job due in `now`'s minute (once per minute, even if called twice).
    pub fn tick(self: &Arc<Self>, now: chrono::DateTime<Local>) {
        {
            let key = (now.ordinal(), now.hour(), now.minute());
            let mut last = self.last_tick.lock().unwrap_or_else(|e| e.into_inner());
            if *last == Some(key) {
                return;
            }
            *last = Some(key);
        }
        let Ok(jobs) = self.db.read(repo::list) else {
            return;
        };
        for job in jobs.into_iter().filter(|j| j.enabled) {
            let due = Cron::parse(&job.cron).is_ok_and(|c| c.matches(&now));
            if !due {
                continue;
            }
            if self.lock_live().contains_key(&job.id) {
                let _ = self.db.write(|c| {
                    repo::record_not_started(
                        c,
                        &job.id,
                        "schedule",
                        RunStatus::Skipped,
                        "Skipped: the previous run was still going.",
                    )
                });
                self.sink.job_changed(&job.id);
                continue;
            }
            let _ = self.launch(&job, "schedule");
        }
    }

    // ---- what a job runs -------------------------------------------------------------

    fn resolve(&self, job: &Job) -> Result<Resolved, Failure> {
        let project = self
            .db
            .read(|c| crate::persistence::projects::get(c, &job.project_id))
            .map_err(|_| invalid("The job's project no longer exists."))?;
        let cwd = PathBuf::from(&project.root_path);
        if !cwd.is_dir() {
            return Err(invalid(format!(
                "The project folder {} doesn't exist.",
                cwd.display()
            )));
        }
        let prefix =
            crate::services::runtime::project_path_prefix(&self.db, &self.catalog, &job.project_id)
                .map_err(internal)?;
        let php = || -> Result<PathBuf, Failure> {
            let sel = self
                .db
                .read(|c| crate::services::runtime::effective(c, Some(&job.project_id), "php"))
                .map_err(internal)?
                .ok_or_else(|| {
                    invalid(
                        "This project has no PHP version. Install PHP or pin one for the project.",
                    )
                })?;
            let exe = Path::new(&sel.install_path).join("php.exe");
            if exe.is_file() {
                Ok(exe)
            } else {
                Err(invalid(format!("PHP {} is missing php.exe.", sel.version)))
            }
        };
        let (exe, args) = match job.kind {
            JobKind::ArtisanSchedule => {
                if !cwd.join("artisan").is_file() {
                    return Err(invalid(
                        "This project has no artisan file, so it isn't a Laravel project.",
                    ));
                }
                (
                    php()?,
                    vec!["artisan".to_owned(), "schedule:run".to_owned()],
                )
            }
            JobKind::PhpScript => {
                let script = job
                    .args
                    .first()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| invalid("Choose the PHP script to run."))?;
                let full = crate::security::paths::join_relative(&cwd, script)
                    .map_err(|_| invalid("The script must be a path inside the project folder."))?;
                if !full.is_file() {
                    return Err(invalid(format!("{script} doesn't exist in the project.")));
                }
                (php()?, job.args.clone())
            }
            JobKind::Command => {
                let program = job
                    .program
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .ok_or_else(|| invalid("Enter the program to run."))?;
                (find_program(program, &prefix)?, job.args.clone())
            }
        };
        Ok(Resolved {
            exe,
            args,
            cwd,
            prefix,
        })
    }

    fn command(&self, r: &Resolved) -> Command {
        let mut cmd = Command::new(&r.exe);
        cmd.args(&self.launcher.prefix);
        if !self.launcher.args_via_env {
            cmd.args(&r.args);
        }
        cmd.env_clear();
        if self.launcher.args_via_env {
            cmd.env("DEVLUMA_FAKE_WORKER_ARGS", r.args.join("\u{1f}"));
        }
        for key in [
            "SystemRoot",
            "SystemDrive",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "USERNAME",
            "COMPUTERNAME",
            "PATHEXT",
        ] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        let mut path: Vec<String> = r.prefix.clone();
        path.extend(system_dirs().iter().map(|d| d.display().to_string()));
        cmd.env("PATH", path.join(";"));
        for (k, v) in &self.launcher.env {
            cmd.env(k, v);
        }
        cmd.current_dir(&r.cwd)
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    // ---- running -----------------------------------------------------------------------

    /// Starts one run now. `trigger` is `schedule` or `manual`.
    pub fn launch(self: &Arc<Self>, job: &Job, trigger: &str) -> Result<Run, Failure> {
        if self.lock_live().contains_key(&job.id) {
            return Err(invalid("This job is already running."));
        }
        let resolved = match self.resolve(job) {
            Ok(r) => r,
            Err(f) => {
                let _ = self.db.write(|c| {
                    repo::record_not_started(c, &job.id, trigger, RunStatus::Failed, &f.message)
                });
                self.sink.job_changed(&job.id);
                return Err(f);
            }
        };
        let run = self
            .db
            .write(|c| repo::start_run(c, &job.id, trigger))
            .map_err(internal)?;
        let finish = |status: RunStatus, message: String| {
            let _ = self
                .db
                .write(|c| repo::finish_run(c, &run.id, status, None, Some(&message)));
            self.sink.job_changed(&job.id);
            failure(ErrorCode::ProcessExited, message)
        };

        let log_path = self.log_path(&job.id, &run.id);
        let log = std::fs::create_dir_all(self.logs_dir(&job.id))
            .and_then(|()| File::create(&log_path))
            .and_then(|mut f| {
                writeln!(
                    f,
                    "> {} {}\r\n  in {}\r\n",
                    resolved.exe.display(),
                    resolved.args.join(" "),
                    resolved.cwd.display()
                )?;
                Ok(f)
            });
        let (out, err) = match log.and_then(|f| f.try_clone().map(|g| (f, g))) {
            Ok(pair) => pair,
            Err(e) => {
                return Err(finish(
                    RunStatus::Failed,
                    format!("Could not create the log: {e}"),
                ));
            }
        };
        let job_obj = match job::Job::create(&job::new_job_name(&format!("sched-{}", run.id)))
            .and_then(|j| j.kill_on_close().map(|()| j))
        {
            Ok(j) => Arc::new(j),
            Err(e) => {
                return Err(finish(
                    RunStatus::Failed,
                    format!("Could not create a job object: {e}"),
                ));
            }
        };
        let mut cmd = self.command(&resolved);
        cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
        let mut child = match job_obj.spawn(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                return Err(finish(
                    RunStatus::Failed,
                    format!("Could not start {}: {e}", resolved.exe.display()),
                ));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.lock_live().insert(
            job.id.clone(),
            Live {
                started_at: run.started_at.clone(),
                job: Arc::clone(&job_obj),
                stop: Arc::clone(&stop),
            },
        );
        self.sink.job_changed(&job.id);

        let timeout = self
            .timeout_override
            .unwrap_or(Duration::from_secs(job.timeout_secs.into()));
        let this = Arc::clone(self);
        let (job_id, run_id) = (job.id.clone(), run.id.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            let (status, code, message) = loop {
                if stop.load(Ordering::SeqCst) {
                    let _ = job_obj.terminate(1);
                    let _ = child.wait();
                    break (RunStatus::Stopped, None, Some("Stopped.".to_owned()));
                }
                match child.try_wait() {
                    Ok(Some(s)) => {
                        let code = s.code().map(i64::from);
                        break if s.success() {
                            (RunStatus::Succeeded, code, None)
                        } else {
                            (
                                RunStatus::Failed,
                                code,
                                Some(format!("Exited with code {}.", code.unwrap_or(-1))),
                            )
                        };
                    }
                    Ok(None) if started.elapsed() >= timeout => {
                        let _ = job_obj.terminate(1);
                        let _ = child.wait();
                        break (
                            RunStatus::TimedOut,
                            None,
                            Some(format!(
                                "Stopped after the {}-second time limit.",
                                timeout.as_secs()
                            )),
                        );
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(e) => break (RunStatus::Failed, None, Some(e.to_string())),
                }
            };
            // Anything the run left behind (background children) ends with its job.
            let _ = job_obj.terminate(1);
            let _ = this
                .db
                .write(|c| repo::finish_run(c, &run_id, status, code, message.as_deref()));
            this.lock_live().remove(&job_id);
            if let Ok(pruned) = this.db.write(|c| repo::prune_runs(c, &job_id)) {
                for id in pruned {
                    let _ = std::fs::remove_file(this.log_path(&job_id, &id));
                }
            }
            this.sink.job_changed(&job_id);
        });
        Ok(run)
    }

    /// Stops the job's run in progress (its whole process tree).
    pub fn stop(&self, job_id: &str) -> Result<(), Failure> {
        let live = self.lock_live();
        let l = live
            .get(job_id)
            .ok_or_else(|| invalid("This job isn't running."))?;
        l.stop.store(true, Ordering::SeqCst);
        let _ = l.job.terminate(1);
        Ok(())
    }

    /// On exit: end every run. Their rows are closed on the next start.
    pub fn stop_all(&self) {
        for l in self.lock_live().values() {
            l.stop.store(true, Ordering::SeqCst);
            let _ = l.job.terminate(1);
        }
    }

    pub fn is_running(&self, job_id: &str) -> bool {
        self.lock_live().contains_key(job_id)
    }

    // ---- jobs --------------------------------------------------------------------------

    fn validate(&self, req: &JobRequest) -> Result<(Job, Cron), Failure> {
        let name = req.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(invalid("Give the job a name (up to 80 characters)."));
        }
        let cron = Cron::parse(&req.cron).map_err(|e| invalid(format!("Schedule: {e}")))?;
        if !(10..=86_400).contains(&req.timeout_secs) {
            return Err(invalid(
                "The time limit must be between 10 seconds and 24 hours.",
            ));
        }
        let args = match req.kind {
            JobKind::ArtisanSchedule => Vec::new(),
            _ => req
                .args
                .iter()
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty())
                .collect(),
        };
        let job = Job {
            id: req.id.clone().unwrap_or_default(),
            project_id: req.project_id.clone(),
            name: name.to_owned(),
            kind: req.kind,
            program: match req.kind {
                JobKind::Command => req.program.as_ref().map(|p| p.trim().to_owned()),
                _ => None,
            },
            args,
            cron: cron.as_str().to_owned(),
            enabled: req.enabled,
            timeout_secs: req.timeout_secs,
            created_at: String::new(),
            updated_at: String::new(),
        };
        Ok((job, cron))
    }

    /// Exactly what the job would run and when — shown for confirmation before saving.
    pub fn preview(&self, req: &JobRequest) -> Result<JobPreview, Failure> {
        let (job, cron) = self.validate(req)?;
        let resolved = self.resolve(&job)?;
        Ok(JobPreview {
            command: resolved.preview(),
            cron: cron.as_str().to_owned(),
            schedule_text: cron.describe(),
            next_runs: cron
                .next_runs(&Local::now(), 3)
                .iter()
                .map(|t| t.to_rfc3339())
                .collect(),
        })
    }

    pub fn save(&self, req: &JobRequest) -> Result<JobView, Failure> {
        let (job, _) = self.validate(req)?;
        self.resolve(&job)?;
        let fields = JobFields {
            project_id: &job.project_id,
            name: &job.name,
            kind: job.kind,
            program: job.program.as_deref(),
            args: &job.args,
            cron: &job.cron,
            enabled: job.enabled,
            timeout_secs: job.timeout_secs,
        };
        let saved = self
            .db
            .write(|c| match &req.id {
                Some(id) => repo::update(c, id, &fields),
                None => repo::insert(c, &fields),
            })
            .map_err(|e| failure(ErrorCode::NotFound, e.to_string()))?;
        self.sink.job_changed(&saved.id);
        self.view(&saved.id)
    }

    pub fn set_enabled(&self, job_id: &str, enabled: bool) -> Result<JobView, Failure> {
        self.db
            .write(|c| repo::set_enabled(c, job_id, enabled))
            .map_err(|e| failure(ErrorCode::NotFound, e.to_string()))?;
        self.sink.job_changed(job_id);
        self.view(job_id)
    }

    pub fn run_now(self: &Arc<Self>, job_id: &str) -> Result<Run, Failure> {
        let job = self
            .db
            .read(|c| repo::get(c, job_id))
            .map_err(|e| failure(ErrorCode::NotFound, e.to_string()))?;
        self.launch(&job, "manual")
    }

    pub fn delete(&self, job_id: &str) -> Result<(), Failure> {
        if self.is_running(job_id) {
            self.stop(job_id)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.is_running(job_id) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.db
            .write(|c| repo::delete(c, job_id))
            .map_err(internal)?;
        let _ = std::fs::remove_dir_all(self.logs_dir(job_id));
        self.sink.job_changed(job_id);
        Ok(())
    }

    pub fn runs(&self, job_id: &str) -> Result<Vec<Run>, Failure> {
        self.db
            .read(|c| repo::runs(c, job_id, repo::KEEP_RUNS))
            .map_err(internal)
    }

    /// The end of a run's output, redacted.
    pub fn run_log(&self, job_id: &str, run_id: &str) -> Result<String, Failure> {
        let run = self
            .db
            .read(|c| repo::get_run(c, run_id))
            .map_err(|e| failure(ErrorCode::NotFound, e.to_string()))?;
        if run.job_id != job_id {
            return Err(failure(
                ErrorCode::NotFound,
                "That run belongs to another job.",
            ));
        }
        let path = self.log_path(job_id, run_id);
        let Ok(mut f) = File::open(&path) else {
            return Ok(String::new());
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len > LOG_TAIL_BYTES {
            let _ = f.seek(SeekFrom::Start(len - LOG_TAIL_BYTES));
        }
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(internal)?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(crate::security::redact::redact(&text).into_owned())
    }

    pub fn list(&self) -> Result<Vec<JobView>, Failure> {
        let jobs = self.db.read(repo::list).map_err(internal)?;
        jobs.iter().map(|j| self.to_view(j)).collect()
    }

    pub fn view(&self, job_id: &str) -> Result<JobView, Failure> {
        let job = self
            .db
            .read(|c| repo::get(c, job_id))
            .map_err(|e| failure(ErrorCode::NotFound, e.to_string()))?;
        self.to_view(&job)
    }

    fn to_view(&self, job: &Job) -> Result<JobView, Failure> {
        let project_name = self
            .db
            .read(|c| crate::persistence::projects::get(c, &job.project_id))
            .map(|p| p.name)
            .unwrap_or_default();
        let last_run = self
            .db
            .read(|c| repo::last_finished(c, &job.id))
            .map_err(internal)?;
        let running_since = self.lock_live().get(&job.id).map(|l| l.started_at.clone());
        let cron = Cron::parse(&job.cron).ok();
        let status = if running_since.is_some() {
            JobStatus::Executing
        } else if !job.enabled {
            JobStatus::Muted
        } else if last_run
            .as_ref()
            .is_some_and(|r| matches!(r.status, RunStatus::Failed | RunStatus::TimedOut))
        {
            JobStatus::Failed
        } else {
            JobStatus::Active
        };
        let (command, problem) = match self.resolve(job) {
            Ok(r) => (Some(r.preview()), None),
            Err(f) => (None, Some(f.message)),
        };
        Ok(JobView {
            id: job.id.clone(),
            project_id: job.project_id.clone(),
            project_name,
            name: job.name.clone(),
            kind: job.kind,
            program: job.program.clone(),
            args: job.args.clone(),
            cron: job.cron.clone(),
            schedule_text: cron
                .as_ref()
                .map(Cron::describe)
                .unwrap_or_else(|| job.cron.clone()),
            enabled: job.enabled,
            timeout_secs: job.timeout_secs,
            status,
            next_run: cron
                .filter(|_| job.enabled)
                .and_then(|c| c.next_runs(&Local::now(), 1).into_iter().next())
                .map(|t| t.to_rfc3339()),
            last_run,
            running_since,
            command,
            problem,
        })
    }
}

fn system_dirs() -> Vec<PathBuf> {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| vec![root.join("System32"), root])
        .unwrap_or_default()
}

/// An absolute path to an existing .exe, or a name found on the project PATH then the system
/// directories. Only .exe files: batch files would need cmd.exe (no shell strings, ADR 0017).
fn find_program(program: &str, prefix: &[String]) -> Result<PathBuf, Failure> {
    let has_dir = program.contains(['\\', '/', ':']);
    if has_dir {
        let p = PathBuf::from(program);
        if !p.is_absolute() {
            return Err(invalid(
                "Give the program as a name (like node) or a full path to an .exe.",
            ));
        }
        if !is_exe(&p) {
            return Err(invalid(format!("{program} is not an .exe file.")));
        }
        if !p.is_file() {
            return Err(invalid(format!("{program} doesn't exist.")));
        }
        return Ok(p);
    }
    let file = if program.to_ascii_lowercase().ends_with(".exe") {
        program.to_owned()
    } else {
        format!("{program}.exe")
    };
    prefix
        .iter()
        .map(PathBuf::from)
        .chain(system_dirs())
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            invalid(format!(
                "Couldn't find {file} on this project's PATH. Install the runtime, or give the full path to an .exe."
            ))
        })
}

fn is_exe(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

#[cfg(test)]
mod tests;
