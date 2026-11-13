//! Composer (ADR 0020). A catalog tool: one verified `composer.phar` per version in
//! `bin\composer\<version>-x64\`, plus a `composer.bat` Devluma writes next to it so
//! `composer …` works in Devluma's terminals with the PHP on their PATH (the project's pin or
//! the default). Composer's home and cache stay inside the Devluma folder.
//!
//! Devluma runs Composer only when the user asks: "composer install/update" on a project,
//! or creating a project from an app (`services::apps`). Each run is journaled, logged to
//! `logs\composer\<operation>.log` and stoppable (kill-on-close job).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::config::{ConfigChange, ConfigService, ConfigTarget};
use super::install::ExecutableProbe;
use super::journal::{Journal, ProgressSink};
use super::runtime::{self, Selection};
use super::tasks::Tasks;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::NewOperation;
use crate::persistence::projects;
use crate::persistence::{Database, DbResult};
use crate::platform::windows::job;
use crate::security::redact;
use crate::state::DataPaths;

pub const PACKAGE_ID: &str = "composer";
pub const PHAR: &str = "composer.phar";
pub const SHIM: &str = "composer.bat";

/// PHP extensions Composer needs on Windows: TLS for packagist.org and GitHub, `zip` to
/// unpack dist archives (there's no `unzip` or git to fall back on), `curl` for parallel
/// downloads and `mbstring` for most packages.
pub const EXTENSIONS: &[&str] = &["openssl", "zip", "curl", "mbstring"];

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `composer.bat`: runs the phar next to it with the php.exe on PATH. `self-update` is
/// refused because it would replace the verified file; newer versions come from the catalog.
const SHIM_TEXT: &str = "@echo off\r\n\
rem Created by Devluma: runs this folder's composer.phar with the php.exe on PATH.\r\n\
if /i \"%~1\"==\"self-update\" goto managed\r\n\
if /i \"%~1\"==\"selfupdate\" goto managed\r\n\
where php.exe >nul 2>nul || (echo Composer needs PHP. Install PHP in Devluma and make it the default, or pin a version for this project. 1>&2 & exit /b 1)\r\n\
php.exe \"%~dp0composer.phar\" %*\r\n\
exit /b %ERRORLEVEL%\r\n\
:managed\r\n\
echo Devluma manages Composer's version. Install a newer one on the Runtimes ^& Packages page. 1>&2\r\n\
exit /b 1\r\n";

/// Writes `composer.bat` into a Composer install folder (called by the installer).
pub fn write_shim(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(SHIM), SHIM_TEXT)
}

/// Composer's home (config, auth.json, global packages) and its cache, inside the root.
pub fn env(paths: &DataPaths) -> Vec<(String, String)> {
    let home = paths.root.join("composer");
    vec![
        ("COMPOSER_HOME".into(), home.display().to_string()),
        (
            "COMPOSER_CACHE_DIR".into(),
            home.join("cache").display().to_string(),
        ),
    ]
}

/// The Composer a project (or the global terminal) uses: its pin or the default, else the
/// newest installed version.
pub fn find(c: &rusqlite::Connection, project_id: Option<&str>) -> DbResult<Option<Selection>> {
    if let Some(sel) = runtime::effective(c, project_id, PACKAGE_ID)? {
        return Ok(Some(sel));
    }
    Ok(newest_installed(c, PACKAGE_ID)?.map(|i| Selection {
        version: i
            .package_version_id
            .split('@')
            .nth(1)
            .unwrap_or_default()
            .to_owned(),
        installation_id: i.id,
        install_path: i.install_path,
        source: runtime::SelectionSource::GlobalDefault,
    }))
}

/// The installed version of `package_id` with the highest version number.
pub fn newest_installed(
    c: &rusqlite::Connection,
    package_id: &str,
) -> DbResult<Option<installations::Installation>> {
    let mut all: Vec<_> = installations::list(c)?
        .into_iter()
        .filter(|i| i.package_id == package_id && i.status == InstallStatus::Installed)
        .collect();
    all.sort_by_key(|i| version_key(i.package_version_id.split('@').nth(1).unwrap_or("")));
    Ok(all.pop())
}

/// "8.4.27-1" → [8, 4, 27, 1] for ordering.
pub fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-'])
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Which of `required` PHP doesn't load with its current php.ini (`php -m`).
pub fn missing_extensions(
    probe: &dyn ExecutableProbe,
    php_install: &Path,
    required: &[&str],
) -> Result<Vec<String>, String> {
    let details = super::php::inspect(probe, php_install)?;
    let loaded: Vec<String> = details
        .loaded_modules
        .iter()
        .map(|m| m.to_ascii_lowercase())
        .collect();
    Ok(required
        .iter()
        .filter(|r| !loaded.iter().any(|m| m == *r))
        .map(|r| (*r).to_owned())
        .collect())
}

/// Turns extensions on in a PHP version's php.ini through the config service (preview,
/// backup, PHP's own check, rollback; journaled as `edit_config`). A missing php.ini is
/// created from the development template first. Returns the running PHP backends that keep
/// the old php.ini until they restart.
pub fn enable_extensions(
    config: &Arc<ConfigService>,
    probe: &dyn ExecutableProbe,
    php: &Selection,
    names: &[String],
) -> Result<Vec<super::config::RestartHint>, IpcError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let install = Path::new(&php.install_path);
    if !install.join("php.ini").is_file() {
        super::php::create_ini(probe, install, super::php::IniTemplate::Development).map_err(
            |e| {
                IpcError::new(
                    ErrorCode::ConfigInvalid,
                    format!("Couldn't create php.ini for PHP {}: {e}", php.version),
                    true,
                )
            },
        )?;
    }
    let extensions: BTreeMap<String, bool> = names.iter().map(|n| (n.clone(), true)).collect();
    let applied = config.apply(
        &ConfigTarget::Php {
            installation_id: php.installation_id.clone(),
        },
        &ConfigChange::Settings {
            values: BTreeMap::new(),
            extensions,
        },
    )?;
    Ok(applied.restart)
}

// ---- running Composer -------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum RunError {
    Cancelled,
    Failed(String),
}

pub struct Run<'a> {
    /// `php.exe` of the PHP version to use.
    pub php_exe: &'a Path,
    pub phar: &'a Path,
    pub args: Vec<OsString>,
    pub cwd: &'a Path,
    pub log: &'a Path,
    pub timeout: Duration,
    pub cancel: &'a AtomicBool,
    pub env: Vec<(String, String)>,
}

fn system_dirs() -> Vec<PathBuf> {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| vec![root.join("System32"), root])
        .unwrap_or_default()
}

/// The environment of a Composer run: a cleared environment with an allowlist (proxy
/// settings pass through), PHP and Composer first on PATH, then system folders only.
fn command(r: &Run<'_>) -> Command {
    let mut cmd = Command::new(r.php_exe);
    cmd.arg(r.phar).args(&r.args).env_clear();
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
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    let mut path: Vec<PathBuf> = Vec::new();
    path.extend(r.php_exe.parent().map(Path::to_path_buf));
    path.extend(r.phar.parent().map(Path::to_path_buf));
    path.extend(system_dirs());
    cmd.env("PATH", std::env::join_paths(path).unwrap_or_default());
    cmd.env("COMPOSER_NO_INTERACTION", "1");
    for (k, v) in &r.env {
        cmd.env(k, v);
    }
    cmd.current_dir(r.cwd)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// The last `n` non-empty lines of the log, redacted (for failure messages).
pub fn log_tail(log: &Path, n: usize) -> String {
    let text = std::fs::read(log)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(n)..].join("\n");
    redact::redact(&tail).into_owned()
}

/// Runs Composer to completion in a kill-on-close job. Output goes to `log`; `on_line` gets
/// the newest output line now and then (for progress).
pub fn run(r: Run<'_>, mut on_line: impl FnMut(&str)) -> Result<(), RunError> {
    if let Some(dir) = r.log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| RunError::Failed(e.to_string()))?;
    }
    let out = std::fs::File::create(r.log).map_err(|e| RunError::Failed(e.to_string()))?;
    let err = out
        .try_clone()
        .map_err(|e| RunError::Failed(e.to_string()))?;
    let job_obj = job::Job::create(&job::new_job_name("composer"))
        .and_then(|j| j.kill_on_close().map(|()| j))
        .map_err(|e| RunError::Failed(format!("Could not create a job object: {e}")))?;
    let mut cmd = command(&r);
    cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
    let mut child = job_obj
        .spawn(&mut cmd)
        .map_err(|e| RunError::Failed(format!("Could not start PHP: {e}")))?;

    let started = Instant::now();
    let mut offset = 0u64;
    let mut last_line = String::new();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let result = loop {
        if r.cancel.load(Ordering::SeqCst) {
            break Err(RunError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                let tail = log_tail(r.log, 12);
                break Err(RunError::Failed(format!(
                    "Composer exited with code {}.{}",
                    status.code().unwrap_or(-1),
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!("\n{tail}")
                    }
                )));
            }
            Ok(None) if started.elapsed() >= r.timeout => {
                break Err(RunError::Failed(format!(
                    "Composer was stopped after {} minutes.",
                    r.timeout.as_secs() / 60
                )));
            }
            Ok(None) => {}
            Err(e) => break Err(RunError::Failed(e.to_string())),
        }
        if last_emit.elapsed() >= Duration::from_millis(700)
            && let Some(line) = newest_line(r.log, &mut offset)
            && line != last_line
        {
            on_line(&line);
            last_line = line;
            last_emit = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    // Anything still running (a script's child, or everything after a stop) ends here.
    let _ = job_obj.terminate(1);
    let _ = child.wait();
    result
}

/// The last complete, non-empty line written since `offset` (which moves forward).
fn newest_line(log: &Path, offset: &mut u64) -> Option<String> {
    let mut f = std::fs::File::open(log).ok()?;
    f.seek(SeekFrom::Start(*offset)).ok()?;
    let mut buf = Vec::new();
    f.take(1_048_576).read_to_end(&mut buf).ok()?;
    let end = buf.iter().rposition(|b| *b == b'\n')?;
    *offset += end as u64 + 1;
    let text = String::from_utf8_lossy(&buf[..end]);
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let line: String = line.chars().take(140).collect();
    Some(redact::redact(&line).into_owned())
}

// ---- project actions --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Install,
    Update,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerStatus {
    /// Composer the project uses, if one is installed.
    pub composer_version: Option<String>,
    pub php_version: Option<String>,
    pub php_installation_id: Option<String>,
    /// Extensions Composer needs that this PHP doesn't load.
    pub missing_extensions: Vec<String>,
    pub has_composer_json: bool,
    pub has_vendor: bool,
    /// A Composer run (or app install) is in progress for this project.
    pub running: bool,
}

pub struct ComposerService {
    db: Arc<Database>,
    paths: DataPaths,
    probe: Arc<dyn ExecutableProbe>,
    sink: Arc<dyn ProgressSink>,
    config: Arc<ConfigService>,
    tasks: Arc<Tasks>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, msg, false)
}

/// What a run needs: Composer, the project's PHP, the project folder.
struct Resolved {
    root: PathBuf,
    composer: Option<Selection>,
    php: Option<Selection>,
}

impl ComposerService {
    pub fn new(
        db: Arc<Database>,
        paths: DataPaths,
        probe: Arc<dyn ExecutableProbe>,
        sink: Arc<dyn ProgressSink>,
        config: Arc<ConfigService>,
        tasks: Arc<Tasks>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            paths,
            probe,
            sink,
            config,
            tasks,
        })
    }

    fn resolve(&self, project_id: &str) -> Result<Resolved, IpcError> {
        Ok(self.db.read(|c| {
            let p = projects::get(c, project_id)?;
            Ok(Resolved {
                root: PathBuf::from(&p.root_path),
                composer: find(c, Some(project_id))?,
                php: runtime::effective(c, Some(project_id), "php")?,
            })
        })?)
    }

    pub fn status(&self, project_id: &str) -> Result<ComposerStatus, IpcError> {
        let r = self.resolve(project_id)?;
        let missing_extensions = match &r.php {
            Some(php) => missing_extensions(&*self.probe, Path::new(&php.install_path), EXTENSIONS)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Ok(ComposerStatus {
            composer_version: r.composer.map(|s| s.version),
            php_version: r.php.as_ref().map(|s| s.version.clone()),
            php_installation_id: r.php.map(|s| s.installation_id),
            missing_extensions,
            has_composer_json: r.root.join("composer.json").is_file(),
            has_vendor: r.root.join("vendor").is_dir(),
            running: self.tasks.busy(&r.root.display().to_string()),
        })
    }

    /// Turns on the extensions Composer needs for the project's PHP version.
    pub fn enable_extensions(&self, project_id: &str) -> Result<(), IpcError> {
        let r = self.resolve(project_id)?;
        let php = r
            .php
            .ok_or_else(|| invalid("This project has no PHP version yet."))?;
        let missing = missing_extensions(&*self.probe, Path::new(&php.install_path), EXTENSIONS)
            .map_err(|e| invalid(format!("Couldn't check PHP {}: {e}", php.version)))?;
        // Composer runs as a new CLI process, so it sees the change at once.
        enable_extensions(&self.config, &*self.probe, &php, &missing).map(|_| ())
    }

    /// Starts `composer install` or `composer update` in the project folder. Returns the
    /// operation id.
    pub fn start(self: &Arc<Self>, project_id: &str, action: Action) -> Result<String, IpcError> {
        let r = self.resolve(project_id)?;
        if !r.root.join("composer.json").is_file() {
            return Err(invalid("This project has no composer.json."));
        }
        let composer = r.composer.ok_or_else(|| {
            invalid("Composer isn't installed. Install it on the Runtimes & Packages page.")
        })?;
        let php = r.php.ok_or_else(|| {
            invalid(
                "Composer needs PHP. Make a PHP version the default, or pin one for this project.",
            )
        })?;
        let missing = missing_extensions(&*self.probe, Path::new(&php.install_path), EXTENSIONS)
            .map_err(|e| invalid(format!("Couldn't check PHP {}: {e}", php.version)))?;
        if !missing.is_empty() {
            return Err(invalid(format!(
                "Composer needs these PHP {} extensions turned on: {}.",
                php.version,
                missing.join(", ")
            )));
        }
        let target = r.root.display().to_string();
        if self.tasks.busy(&target) {
            return Err(invalid("Composer is already running for this project."));
        }
        let journal = Journal::new(&self.db, self.sink.as_ref());
        let op = journal.start(&NewOperation {
            operation_type: match action {
                Action::Install => "composer_install",
                Action::Update => "composer_update",
            },
            target_type: Some("project"),
            target_id: Some(project_id),
        })?;
        let Some(cancel) = self.tasks.register(&op.id, &target) else {
            let _ = journal.fail(&op.id, ErrorCode::ValidationError, "Already running.");
            return Err(invalid("Composer is already running for this project."));
        };
        let (me, op_id) = (Arc::clone(self), op.id.clone());
        std::thread::spawn(move || {
            let journal = Journal::new(&me.db, me.sink.as_ref());
            let _ = journal.running(&op_id);
            let log = me.log_path(&op_id);
            let result = run(
                Run {
                    php_exe: &Path::new(&php.install_path).join("php.exe"),
                    phar: &Path::new(&composer.install_path).join(PHAR),
                    args: vec![
                        action.verb().into(),
                        "--no-interaction".into(),
                        "--no-ansi".into(),
                    ],
                    cwd: &r.root,
                    log: &log,
                    timeout: Duration::from_secs(30 * 60),
                    cancel: &cancel,
                    env: env(&me.paths),
                },
                |line| {
                    let _ = journal.progress(&op_id, line, None, None);
                },
            );
            let _ = match result {
                Ok(()) => journal.succeed(&op_id).map(|_| ()),
                Err(RunError::Cancelled) => journal.cancel(&op_id).map(|_| ()),
                Err(RunError::Failed(msg)) => journal
                    .fail(&op_id, ErrorCode::ProcessExited, &msg)
                    .map(|_| ()),
            };
            me.tasks.finish(&op_id);
        });
        Ok(op.id)
    }

    pub fn log_path(&self, operation_id: &str) -> PathBuf {
        log_path(&self.paths, operation_id)
    }
}

/// `logs\composer\<operation>.log` (operation ids are ULIDs).
pub fn log_path(paths: &DataPaths, operation_id: &str) -> PathBuf {
    paths
        .root
        .join("logs")
        .join("composer")
        .join(format!("{operation_id}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_runs_the_phar_beside_it_and_refuses_self_update() {
        let dir = tempfile::tempdir().unwrap();
        write_shim(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(SHIM)).unwrap();
        assert!(text.contains("php.exe \"%~dp0composer.phar\" %*\r\n"));
        assert!(text.contains("\"self-update\" goto managed"));
        assert!(text.lines().all(|l| !l.contains('\n')));
    }

    /// The real shim with a stand-in php.exe (cmd's own `where` finds it on PATH).
    #[test]
    fn shim_passes_arguments_and_exit_codes_through() {
        let dir = tempfile::tempdir().unwrap();
        write_shim(dir.path()).unwrap();
        let cmd_exe = PathBuf::from(std::env::var("SystemRoot").unwrap()).join(r"System32\cmd.exe");
        let run_shim = |path: &str, args: &[&str]| {
            std::process::Command::new(&cmd_exe)
                .args(["/d", "/c"])
                .arg(dir.path().join(SHIM))
                .args(args)
                .env("PATH", path)
                .output()
                .unwrap()
        };
        let system = format!(r"{}\System32", std::env::var("SystemRoot").unwrap());
        let out = run_shim(&system, &["install"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("Composer needs PHP"));
        let out = run_shim(&system, &["self-update"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("manages Composer"));
    }

    #[test]
    fn versions_order_numerically() {
        let mut v = vec!["8.4.9", "8.4.27-1", "8.4.27", "8.10.0"];
        v.sort_by_key(|s| version_key(s));
        assert_eq!(v, ["8.4.9", "8.4.27", "8.4.27-1", "8.10.0"]);
    }

    #[test]
    fn home_and_cache_stay_inside_the_root() {
        let paths = DataPaths::under(Path::new(r"D:\Devluma"));
        let env = env(&paths);
        assert_eq!(
            env[0],
            ("COMPOSER_HOME".into(), r"D:\Devluma\composer".into())
        );
        assert_eq!(
            env[1],
            (
                "COMPOSER_CACHE_DIR".into(),
                r"D:\Devluma\composer\cache".into()
            )
        );
    }

    #[test]
    fn newest_line_reads_only_complete_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("x.log");
        std::fs::write(
            &log,
            "a\r\n  - Installing laravel/framework (v13.2.0)\r\npart",
        )
        .unwrap();
        let mut off = 0;
        assert_eq!(
            newest_line(&log, &mut off).as_deref(),
            Some("- Installing laravel/framework (v13.2.0)")
        );
        assert_eq!(newest_line(&log, &mut off), None);
        std::fs::write(
            &log,
            "a\r\n  - Installing laravel/framework (v13.2.0)\r\npart done\n",
        )
        .unwrap();
        assert_eq!(newest_line(&log, &mut off).as_deref(), Some("part done"));
    }
}
