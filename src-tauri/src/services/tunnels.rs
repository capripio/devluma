//! ngrok tunnels for sites (ADR 0011): Devluma never downloads ngrok. The user installs
//! `ngrok.exe`; Devluma finds or is pointed at it, keeps the authtoken in Windows Credential
//! Manager, and runs one tunnel per site on request.
//!
//! - The token reaches ngrok only through `NGROK_AUTHTOKEN` in a cleared environment —
//!   never on the command line, in Devluma's database or in its logs.
//! - Devluma passes its own `--config`, so the user's own ngrok.yml (and any token in it)
//!   isn't used.
//! - Each tunnel runs in a kill-on-close job: it ends when Devluma exits or crashes, so a
//!   site is never left public without Devluma showing it.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::journal::{Journal, ProgressSink};
use super::supervisor::behavior::{Failure, failure};
use crate::error::ErrorCode;
use crate::persistence::operations::NewOperation;
use crate::persistence::{Database, settings, sites};
use crate::platform::windows::{credential, job};
use crate::state::DataPaths;

/// Where older builds kept the token (Windows Credential Manager); moved into the folder on
/// first use (ADR 0014).
pub const TOKEN_TARGET: &str = "Devluma/ngrok/authtoken";
/// The token's file inside Devluma's folder, encrypted with DPAPI for the current Windows
/// account and readable only by it (ACL). Travels with a moved folder; another account or PC
/// can't decrypt it.
pub const TOKEN_FILE: &str = r"secrets\ngrok-authtoken.dpapi";
const PATH_SETTING: &str = "ngrok.path";
const START_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NgrokStatus {
    /// The ngrok.exe Devluma uses, if one was chosen.
    pub path: Option<String>,
    pub version: Option<String>,
    /// Why the chosen ngrok.exe can't be used right now.
    pub problem: Option<String>,
    pub token_saved: bool,
    /// ngrok.exe files found in common places (PATH, winget, Scoop, Chocolatey, Laragon).
    pub detected: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelView {
    pub site_id: String,
    pub domain: String,
    pub public_url: Option<String>,
    pub running: bool,
    pub started_at: String,
    /// The last thing ngrok reported if the tunnel stopped by itself.
    pub last_error: Option<String>,
}

struct Tunnel {
    job: Arc<job::Job>,
    domain: String,
    started_at: String,
    url: Arc<Mutex<Option<String>>>,
    /// Recent ngrok log lines (redacted), newest last.
    tail: Arc<Mutex<VecDeque<String>>>,
}

/// How ngrok is invoked. Tests replace it with a fake process.
#[derive(Clone, Default)]
pub struct Launcher {
    /// Arguments placed before ngrok's own (tests: the test harness filter).
    pub prefix: Vec<String>,
    /// Extra environment (tests only).
    pub env: Vec<(String, String)>,
    /// Tests: pass ngrok's arguments in `DEVLUMA_FAKE_NGROK_ARGS` instead of the command
    /// line (the test harness would reject ngrok's flags).
    pub args_via_env: bool,
}

pub struct TunnelService {
    db: Arc<Database>,
    paths: DataPaths,
    progress: Arc<dyn ProgressSink>,
    token_target: String,
    launcher: Launcher,
    live: Mutex<HashMap<String, Tunnel>>,
}

fn fail(code: ErrorCode, msg: impl Into<String>) -> Failure {
    failure(code, msg)
}

fn db_fail(e: crate::persistence::DbError) -> Failure {
    let i = crate::error::IpcError::from(e);
    failure(i.code, i.message)
}

/// `ngrok version` → "ngrok version 3.35.0".
pub fn parse_version(output: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?m)^ngrok version (\d+\.\d+\.\d+)").ok()?;
    re.captures(output).map(|c| c[1].to_owned())
}

/// ngrok authtokens are long strings of letters, digits and `_`.
fn check_token(token: &str) -> Result<(), Failure> {
    let ok = (20..=200).contains(&token.len())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(fail(
            ErrorCode::ValidationError,
            "That doesn't look like an ngrok authtoken. Copy it from your ngrok dashboard (Your Authtoken).",
        ))
    }
}

/// The public URL from one line of ngrok's JSON log, if the line announces a tunnel.
fn url_from_log(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let url = v.get("url")?.as_str()?;
    url.starts_with("https://").then(|| url.to_owned())
}

/// The most useful error in ngrok's recent log lines.
fn error_from_log(lines: &VecDeque<String>) -> Option<String> {
    lines.iter().rev().find_map(|l| {
        let v: serde_json::Value = serde_json::from_str(l).ok()?;
        v.get("err")
            .and_then(|e| e.as_str())
            .filter(|e| !e.is_empty() && *e != "<nil>")
            .map(str::to_owned)
    })
}

impl TunnelService {
    pub fn new(db: Arc<Database>, paths: DataPaths, progress: Arc<dyn ProgressSink>) -> Arc<Self> {
        Arc::new(Self::with(
            db,
            paths,
            progress,
            TOKEN_TARGET.to_owned(),
            Launcher::default(),
        ))
    }

    pub fn with(
        db: Arc<Database>,
        paths: DataPaths,
        progress: Arc<dyn ProgressSink>,
        token_target: String,
        launcher: Launcher,
    ) -> Self {
        Self {
            db,
            paths,
            progress,
            token_target,
            launcher,
            live: Mutex::new(HashMap::new()),
        }
    }

    fn command(&self, exe: &Path, args: &[String]) -> Command {
        let mut cmd = Command::new(exe);
        cmd.args(&self.launcher.prefix);
        if !self.launcher.args_via_env {
            cmd.args(args);
        }
        cmd.env_clear();
        for key in [
            "SystemRoot",
            "SystemDrive",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
        ] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        if let Some(root) = std::env::var_os("SystemRoot") {
            cmd.env("PATH", PathBuf::from(root).join("System32"));
        }
        for (k, v) in &self.launcher.env {
            cmd.env(k, v);
        }
        if self.launcher.args_via_env {
            cmd.env("DEVLUMA_FAKE_NGROK_ARGS", args.join("\u{1f}"));
        }
        cmd
    }

    /// Runs `ngrok version` (10 s limit) and checks it's ngrok 3.
    fn probe(&self, exe: &Path) -> Result<String, String> {
        let mut cmd = self.command(exe, &["version".to_owned()]);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("Could not run it: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    return Err("It didn't answer `ngrok version` in time.".into());
                }
            }
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        match parse_version(&text) {
            Some(v) if v.starts_with("3.") => Ok(v),
            Some(v) => Err(format!("ngrok {v} is too old; Devluma needs ngrok 3.")),
            None => Err("It doesn't report an ngrok version.".into()),
        }
    }

    fn saved_path(&self) -> Result<Option<PathBuf>, Failure> {
        Ok(self
            .db
            .read(|c| settings::get::<String>(c, PATH_SETTING))
            .map_err(db_fail)?
            .map(PathBuf::from))
    }

    /// ngrok.exe files in common install locations, for the user to pick from.
    pub fn detect() -> Vec<String> {
        let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.join("ngrok.exe"))
                    .collect()
            })
            .unwrap_or_default();
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        if let Some(l) = env("LOCALAPPDATA") {
            candidates.push(l.join(r"Microsoft\WinGet\Links\ngrok.exe"));
            candidates.push(l.join(r"Microsoft\WindowsApps\ngrok.exe"));
        }
        if let Some(u) = env("USERPROFILE") {
            candidates.push(u.join(r"scoop\shims\ngrok.exe"));
        }
        if let Some(p) = env("ProgramData") {
            candidates.push(p.join(r"chocolatey\bin\ngrok.exe"));
        }
        for drive in ["C:", "D:"] {
            candidates.push(PathBuf::from(format!(
                r"{drive}\laragon\bin\ngrok\ngrok.exe"
            )));
        }
        let mut found: Vec<String> = Vec::new();
        for c in candidates {
            let s = c.display().to_string();
            if c.is_file() && !found.iter().any(|f| f.eq_ignore_ascii_case(&s)) {
                found.push(s);
            }
        }
        found
    }

    pub fn status(&self) -> Result<NgrokStatus, Failure> {
        let path = self.saved_path()?;
        let (version, problem) = match &path {
            None => (None, None),
            Some(p) if !p.is_file() => (None, Some(format!("{} no longer exists.", p.display()))),
            Some(p) => match self.probe(p) {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e)),
            },
        };
        Ok(NgrokStatus {
            path: path.map(|p| p.display().to_string()),
            version,
            problem,
            token_saved: self.read_token().is_some(),
            detected: Self::detect(),
        })
    }

    /// Uses `path` as ngrok.exe after checking it is one (by name and `ngrok version`).
    pub fn set_path(&self, path: &str) -> Result<NgrokStatus, Failure> {
        let p = PathBuf::from(path.trim());
        let named_ngrok = p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("ngrok.exe"));
        if !p.is_absolute() || !named_ngrok || !p.is_file() {
            return Err(fail(
                ErrorCode::ValidationError,
                "Choose the full path of an existing ngrok.exe.",
            ));
        }
        self.probe(&p).map_err(|e| {
            fail(
                ErrorCode::ValidationError,
                format!("{} can't be used: {e}", p.display()),
            )
        })?;
        let s = p.display().to_string();
        self.db
            .write(|c| settings::set(c, PATH_SETTING, &s))
            .map_err(db_fail)?;
        self.status()
    }

    fn token_file(&self) -> PathBuf {
        self.paths.root.join(TOKEN_FILE)
    }

    /// Encrypts with DPAPI and writes the token file; the file is restricted to the current
    /// user before any secret bytes are written.
    fn write_token(&self, token: &str) -> Result<(), Failure> {
        let io = |e: std::io::Error| {
            fail(
                ErrorCode::Internal,
                format!("Could not store the token: {e}"),
            )
        };
        let sealed = crate::platform::windows::dpapi::protect(token.as_bytes()).map_err(io)?;
        let file = self.token_file();
        std::fs::create_dir_all(file.parent().unwrap_or(&self.paths.root)).map_err(io)?;
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, b"").map_err(io)?;
        crate::platform::windows::acl::restrict_to_current_user(&tmp).map_err(io)?;
        std::fs::write(&tmp, sealed).map_err(io)?;
        std::fs::rename(&tmp, &file).map_err(io)?;
        self.db
            .write(|c| {
                crate::persistence::secrets::upsert(c, "dpapi_file", TOKEN_FILE, "ngrok authtoken")
            })
            .map_err(db_fail)
    }

    /// The saved token, if any. A token left in Credential Manager by an older build is
    /// moved into the folder first. Registered for redaction whenever it's read.
    fn read_token(&self) -> Option<String> {
        if !self.token_file().exists()
            && let Some(old) =
                credential::read(&self.token_target).and_then(|t| String::from_utf8(t).ok())
            && self.write_token(&old).is_ok()
        {
            credential::delete(&self.token_target);
            let target = self.token_target.clone();
            let _ = self
                .db
                .write(|c| crate::persistence::secrets::delete(c, &target));
        }
        let sealed = std::fs::read(self.token_file()).ok()?;
        let token =
            String::from_utf8(crate::platform::windows::dpapi::unprotect(&sealed).ok()?).ok()?;
        crate::security::redact::register_secret(&token);
        Some(token)
    }

    /// Stores the authtoken in Devluma's folder, encrypted for this Windows account.
    pub fn set_token(&self, token: &str) -> Result<NgrokStatus, Failure> {
        let token = token.trim();
        check_token(token)?;
        crate::security::redact::register_secret(token);
        self.write_token(token)?;
        self.status()
    }

    pub fn remove_token(&self) -> Result<NgrokStatus, Failure> {
        let _ = std::fs::remove_file(self.token_file());
        credential::delete(&self.token_target);
        self.db
            .write(|c| crate::persistence::secrets::delete(c, TOKEN_FILE))
            .map_err(db_fail)?;
        self.status()
    }

    /// Starts a tunnel for a site and waits (up to 30 s) for ngrok's public URL.
    pub fn start(&self, site_id: &str) -> Result<TunnelView, Failure> {
        if let Some(v) = self.view(site_id).filter(|v| v.running) {
            return Ok(v);
        }
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let op = journal
            .start(&NewOperation {
                operation_type: "start_tunnel",
                target_type: Some("site"),
                target_id: Some(site_id),
            })
            .map_err(db_fail)?;
        let _ = journal.running(&op.id);
        match self.do_start(site_id) {
            Ok(v) => {
                let _ = journal.succeed(&op.id);
                Ok(v)
            }
            Err(f) => {
                let _ = journal.fail(&op.id, f.code, &f.message);
                Err(f)
            }
        }
    }

    fn do_start(&self, site_id: &str) -> Result<TunnelView, Failure> {
        let site = self.db.read(|c| sites::get(c, site_id)).map_err(db_fail)?;
        let exe = self.saved_path()?.filter(|p| p.is_file()).ok_or_else(|| {
            fail(
                ErrorCode::ValidationError,
                "Choose your ngrok.exe in Settings first.",
            )
        })?;
        let token = self.read_token().ok_or_else(|| {
            fail(
                ErrorCode::ValidationError,
                "Add your ngrok authtoken in Settings first.",
            )
        })?;
        // An almost empty config: keeps the user's own ngrok.yml (and its token) out of it.
        let dir = self.paths.root.join("tunnels");
        std::fs::create_dir_all(&dir).map_err(|e| fail(ErrorCode::Internal, e.to_string()))?;
        let config = dir.join("ngrok.yml");
        std::fs::write(&config, "version: \"2\"\n")
            .map_err(|e| fail(ErrorCode::Internal, e.to_string()))?;

        let job = Arc::new(
            job::Job::create(&job::new_job_name(&format!("tunnel-{site_id}")))
                .and_then(|j| j.kill_on_close().map(|()| j))
                .map_err(|e| {
                    fail(
                        ErrorCode::Internal,
                        format!("Could not create a job object: {e}"),
                    )
                })?,
        );
        let args = vec![
            "http".to_owned(),
            format!("http://127.0.0.1:{}", site.http_port),
            // Name-based sites: forward with the site's own Host header.
            format!("--host-header={}", site.domain),
            "--log".to_owned(),
            "stdout".to_owned(),
            "--log-format".to_owned(),
            "json".to_owned(),
            "--config".to_owned(),
            config.display().to_string(),
        ];
        let mut cmd = self.command(&exe, &args);
        cmd.env("NGROK_AUTHTOKEN", &token)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = job.spawn(&mut cmd).map_err(|e| {
            fail(
                ErrorCode::ProcessExited,
                format!("Could not start ngrok: {e}"),
            )
        })?;

        let url = Arc::new(Mutex::new(None::<String>));
        let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let pipes: [Box<dyn std::io::Read + Send>; 2] = [
            Box::new(child.stdout.take().expect("piped")),
            Box::new(child.stderr.take().expect("piped")),
        ];
        for pipe in pipes {
            let (url, tail) = (Arc::clone(&url), Arc::clone(&tail));
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if let Some(u) = url_from_log(&line) {
                        url.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_insert(u);
                    }
                    let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                    t.push_back(crate::security::redact::redact(&line).into_owned());
                    if t.len() > 40 {
                        t.pop_front();
                    }
                }
            });
        }

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if url.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                break;
            }
            let exited = matches!(child.try_wait(), Ok(Some(_)));
            if exited || Instant::now() >= deadline {
                let _ = job.terminate(1);
                std::thread::sleep(Duration::from_millis(100)); // let the readers drain
                let why = error_from_log(&tail.lock().unwrap_or_else(|e| e.into_inner()));
                return Err(fail(
                    ErrorCode::ProcessExited,
                    match (exited, why) {
                        (_, Some(e)) => format!("ngrok couldn't start the tunnel: {e}"),
                        (true, None) => "ngrok exited before the tunnel was ready.".to_owned(),
                        (false, None) => {
                            "ngrok didn't report a public URL within 30 seconds.".to_owned()
                        }
                    },
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(child); // the job owns the process from here
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(
            site_id.to_owned(),
            Tunnel {
                job,
                domain: site.domain.clone(),
                started_at: chrono::Utc::now().to_rfc3339(),
                url,
                tail,
            },
        );
        Ok(self.view(site_id).expect("just inserted"))
    }

    pub fn stop(&self, site_id: &str) -> Result<(), Failure> {
        let tunnel = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(site_id);
        if let Some(t) = tunnel {
            let _ = t.job.terminate(0);
            if !t.job.wait_empty(Duration::from_secs(5)) {
                return Err(fail(ErrorCode::ProcessExited, "ngrok didn't stop."));
            }
        }
        Ok(())
    }

    fn view(&self, site_id: &str) -> Option<TunnelView> {
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let t = live.get(site_id)?;
        let running = t.job.process_ids().is_ok_and(|p| !p.is_empty());
        Some(TunnelView {
            site_id: site_id.to_owned(),
            domain: t.domain.clone(),
            public_url: t.url.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            running,
            started_at: t.started_at.clone(),
            last_error: if running {
                None
            } else {
                error_from_log(&t.tail.lock().unwrap_or_else(|e| e.into_inner()))
                    .or_else(|| Some("ngrok stopped.".to_owned()))
            },
        })
    }

    pub fn list(&self) -> Vec<TunnelView> {
        let ids: Vec<String> = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        let mut views: Vec<TunnelView> = ids.iter().filter_map(|id| self.view(id)).collect();
        views.sort_by(|a, b| a.domain.cmp(&b.domain));
        views
    }

    /// Stops every tunnel (app exit; jobs would also end them when Devluma's handles close).
    pub fn stop_all(&self) {
        let ids: Vec<String> = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.stop(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;
    use crate::services::journal::test_support::RecordingSink;

    const FAKE_TOKEN: &str = "2abcDEFghiJKLmnoPQRstu_vwxYZ0123456789";

    /// Stand-in for ngrok.exe: the test binary re-run as this test. Its real arguments come
    /// through `DEVLUMA_FAKE_NGROK_ARGS` (the test harness would reject ngrok's flags).
    #[test]
    #[ignore = "fake ngrok process for tunnel tests"]
    fn fake_ngrok() {
        let Ok(args) = std::env::var("DEVLUMA_FAKE_NGROK_ARGS") else {
            return;
        };
        crate::platform::windows::job::tests::exit_with_parent();
        let args: Vec<&str> = args.split('\u{1f}').collect();
        if args == ["version"] {
            println!("ngrok version 3.35.0");
            return;
        }
        let token = std::env::var("NGROK_AUTHTOKEN").unwrap_or_default();
        // What the tests check: the token arrives only via the environment.
        println!(
            "{}",
            serde_json::json!({"lvl": "info", "msg": "fake", "args": args, "auth_from_env_ok": token == FAKE_TOKEN})
        );
        if token != FAKE_TOKEN {
            println!(
                "{}",
                serde_json::json!({"lvl": "eror", "msg": "failed to auth", "err": "authentication failed: Your authtoken is invalid."})
            );
            std::process::exit(1);
        }
        println!(
            "{}",
            serde_json::json!({"lvl": "info", "msg": "started tunnel", "name": "command_line", "addr": args[1], "url": "https://fake-devluma.ngrok-free.app"})
        );
        std::thread::sleep(Duration::from_secs(120));
    }

    struct Harness {
        _dir: tempfile::TempDir,
        svc: TunnelService,
        site_id: String,
        fake_exe: PathBuf,
        target: String,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.svc.stop_all();
            credential::delete(&self.target);
        }
    }

    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(&dir.path().join("Devluma"));
        paths.create_all().unwrap();
        let db = Arc::new(db());
        let project_dir = dir.path().join("shop");
        std::fs::create_dir(&project_dir).unwrap();
        let project =
            crate::services::projects::create(&db, &paths, "Shop", &project_dir.to_string_lossy())
                .unwrap();
        let site_id =
            crate::persistence::sites::test_support::insert(&db, &project.id, "shop.test", 8080);
        // The service checks the file is named ngrok.exe; a copy of the test binary is.
        let fake_exe = dir.path().join("ngrok.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &fake_exe).unwrap();
        let target = format!("Devluma/test-ngrok/{}", ulid::Ulid::generate());
        let launcher = Launcher {
            prefix: vec![
                "--exact".into(),
                "services::tunnels::tests::fake_ngrok".into(),
                "--ignored".into(),
                "--nocapture".into(),
                "--quiet".into(),
            ],
            env: vec![("DEVLUMA_TEST_PARENT".into(), std::process::id().to_string())],
            args_via_env: true,
        };
        let svc = TunnelService::with(
            db,
            paths,
            Arc::new(RecordingSink::default()),
            target.clone(),
            launcher,
        );
        Harness {
            _dir: dir,
            svc,
            site_id,
            fake_exe,
            target,
        }
    }

    #[test]
    fn parses_versions_tokens_and_log_lines() {
        assert_eq!(
            parse_version("ngrok version 3.35.0\r\n").as_deref(),
            Some("3.35.0")
        );
        assert_eq!(parse_version("hello"), None);
        assert!(check_token(FAKE_TOKEN).is_ok());
        assert!(check_token("short").is_err());
        assert!(check_token("has spaces in it that are not allowed!!").is_err());
        assert_eq!(
            url_from_log(
                r#"{"lvl":"info","msg":"started tunnel","url":"https://a.ngrok-free.app"}"#
            )
            .as_deref(),
            Some("https://a.ngrok-free.app")
        );
        assert_eq!(url_from_log(r#"{"url":"tcp://0.tcp.ngrok.io:1"}"#), None);
        assert_eq!(url_from_log("not json"), None);
    }

    #[test]
    fn ngrok_path_must_be_a_working_ngrok_3() {
        let h = harness();
        assert!(
            h.svc.set_path(r"C:\Windows\System32\notepad.exe").is_err(),
            "wrong name"
        );
        assert!(h.svc.set_path("ngrok.exe").is_err(), "relative");
        let status = h.svc.set_path(&h.fake_exe.to_string_lossy()).unwrap();
        assert_eq!(status.version.as_deref(), Some("3.35.0"));
        assert_eq!(status.problem, None);
        assert!(!status.token_saved);
    }

    #[test]
    fn token_is_stored_encrypted_in_the_folder_and_never_in_plain_text() {
        let h = harness();
        assert!(h.svc.set_token("nope").is_err());
        let status = h.svc.set_token(&format!("  {FAKE_TOKEN}\n")).unwrap();
        assert!(status.token_saved);
        let file = h.svc.token_file();
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !bytes
                .windows(FAKE_TOKEN.len())
                .any(|w| w == FAKE_TOKEN.as_bytes()),
            "not plain text"
        );
        assert_eq!(
            crate::platform::windows::acl::dacl_entry_count(&file).unwrap(),
            1,
            "current user only"
        );
        assert_eq!(h.svc.read_token().as_deref(), Some(FAKE_TOKEN));
        // The database holds only the reference.
        let dump: String = h
            .svc
            .db
            .read(|c| {
                let mut stmt =
                    c.prepare("SELECT provider || opaque_key || label FROM secret_references")?;
                let rows: Vec<String> = stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?;
                Ok(rows.join("|"))
            })
            .unwrap();
        assert!(
            dump.contains("dpapi_file") && !dump.contains(FAKE_TOKEN),
            "{dump}"
        );
        assert!(!h.svc.remove_token().unwrap().token_saved);
        assert!(!file.exists());
    }

    #[test]
    fn a_token_left_in_credential_manager_moves_into_the_folder() {
        let h = harness();
        credential::write(&h.target, "ngrok", FAKE_TOKEN.as_bytes()).unwrap();
        assert!(h.svc.status().unwrap().token_saved);
        assert!(h.svc.token_file().is_file());
        assert_eq!(
            credential::read(&h.target),
            None,
            "removed from Credential Manager"
        );
    }

    #[test]
    fn starts_a_tunnel_with_the_token_in_the_environment_only() {
        let h = harness();
        h.svc.set_path(&h.fake_exe.to_string_lossy()).unwrap();
        assert!(
            h.svc
                .start(&h.site_id)
                .unwrap_err()
                .message
                .contains("authtoken"),
            "needs a token first"
        );
        h.svc.set_token(FAKE_TOKEN).unwrap();
        let view = h.svc.start(&h.site_id).unwrap();
        assert_eq!(
            view.public_url.as_deref(),
            Some("https://fake-devluma.ngrok-free.app")
        );
        assert!(view.running);
        let log = h.svc.live.lock().unwrap()[&h.site_id]
            .tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(log.contains("\"auth_from_env_ok\":true"), "{log}");
        // The exact command line: loopback target, the site's Host header, Devluma's config.
        let config = h
            .svc
            .paths
            .root
            .join("tunnels")
            .join("ngrok.yml")
            .display()
            .to_string();
        let expected = serde_json::json!([
            "http",
            "http://127.0.0.1:8080",
            "--host-header=shop.test",
            "--log",
            "stdout",
            "--log-format",
            "json",
            "--config",
            config
        ]);
        assert!(log.contains(&format!("\"args\":{expected}")), "{log}");
        assert!(!log.contains(FAKE_TOKEN), "token never appears in logs");
        assert_eq!(h.svc.list().len(), 1);

        let pid_job = Arc::clone(&h.svc.live.lock().unwrap()[&h.site_id].job);
        h.svc.stop(&h.site_id).unwrap();
        assert!(pid_job.process_ids().unwrap().is_empty());
        assert!(h.svc.list().is_empty());
    }

    #[test]
    fn reports_ngrok_errors() {
        let h = harness();
        h.svc.set_path(&h.fake_exe.to_string_lossy()).unwrap();
        h.svc.set_token("wrong_token_value_1234567890").unwrap();
        let err = h.svc.start(&h.site_id).unwrap_err();
        assert!(
            err.message.contains("authentication failed"),
            "{}",
            err.message
        );
        assert!(h.svc.list().is_empty());
    }
}
