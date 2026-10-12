//! Per-adapter service behaviour: config generation, native validation, launch command,
//! graceful stop and log locations. Everything here is compiled code — catalog data never
//! contributes commands or arguments.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::domain::adapter::AdapterId;
use crate::error::ErrorCode;
use crate::platform::windows::process as winproc;

/// Everything a behaviour needs to know about one service instance.
pub struct ServiceCtx {
    pub service_id: String,
    pub install_dir: PathBuf,
    /// App-managed: `<root>/services/<id>` (generated config, logs, temp).
    pub service_dir: PathBuf,
    /// User-owned data folder (databases).
    pub data_dir: Option<PathBuf>,
    pub port: u16,
    /// Second port for the tool's own UI, when `ServiceBehavior::admin_port` says it has one.
    pub admin_port: Option<u16>,
}

impl ServiceCtx {
    pub fn log_dir(&self) -> PathBuf {
        self.service_dir.join("logs")
    }

    /// Combined stdout/stderr of the service process.
    pub fn console_log(&self) -> PathBuf {
        self.log_dir().join("service.log")
    }
}

pub struct Launch {
    pub exe: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

pub fn failure(code: ErrorCode, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}

pub trait ServiceBehavior: Send + Sync {
    /// The main executable; adopted processes must match it.
    fn image(&self, ctx: &ServiceCtx) -> PathBuf;
    /// Writes Devluma-managed config. Never touches files the user owns.
    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure>;
    /// The file shown on the Configuration tab.
    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf;
    /// Runs the service's own validator where one exists.
    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure>;
    fn launch(&self, ctx: &ServiceCtx) -> Launch;
    /// Asks the service to shut down cleanly. Returns false if the request couldn't be sent.
    fn request_stop(&self, ctx: &ServiceCtx, pid: u32) -> bool;
    fn ready_timeout(&self) -> Duration;
    /// Extra log files beyond the console log, with labels.
    fn log_files(&self, ctx: &ServiceCtx) -> Vec<(String, PathBuf)>;
    /// Label and default of a second port for the tool's own UI, if it has one.
    fn admin_port(&self) -> Option<AdminPort> {
        None
    }
    /// Databases: whether `dir` already holds this engine's data.
    fn is_data_initialized(&self, _dir: &Path) -> bool {
        false
    }
    /// Databases: creates a new data folder (which must be empty or missing).
    fn initialize_data(
        &self,
        _install_dir: &Path,
        _data_dir: &Path,
        _port: u16,
    ) -> Result<(), Failure> {
        Ok(())
    }
    /// Re-reads configuration without dropping connections, if the service supports it.
    fn reload(&self, _ctx: &ServiceCtx) -> Result<(), Failure> {
        Err(failure(
            ErrorCode::NotImplemented,
            "This service can't reload its configuration; restart it instead.",
        ))
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminPort {
    pub label: &'static str,
    pub default: u16,
}

/// Fails unless `dir` is missing or empty: data initialization never writes over files.
pub fn ensure_empty_data_dir(dir: &Path) -> Result<(), Failure> {
    if dir.exists()
        && std::fs::read_dir(dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(failure(
            ErrorCode::DataDirectoryExists,
            format!(
                "{} already contains files. Devluma won't initialize over existing data; choose an empty folder or use the existing data.",
                dir.display()
            ),
        ));
    }
    Ok(())
}

pub fn for_adapter(adapter: AdapterId) -> Option<&'static dyn ServiceBehavior> {
    match adapter {
        AdapterId::Nginx => Some(&Nginx),
        AdapterId::Mariadb => Some(&Mariadb),
        AdapterId::Php => Some(&PhpCgi),
        AdapterId::Mailpit => Some(&Mailpit),
        AdapterId::Postgresql => Some(&Postgresql),
        AdapterId::Mongodb => Some(&Mongodb),
        AdapterId::Apache => Some(&Apache),
        AdapterId::Node | AdapterId::Python => None,
        #[cfg(test)]
        AdapterId::Fixture => Some(&tests_support::Fixture),
    }
}

/// Paths in nginx config use forward slashes; quote everything.
fn ng(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn io_fail(context: &str) -> impl FnOnce(std::io::Error) -> Failure + '_ {
    move |e| failure(ErrorCode::Internal, format!("Could not {context}: {e}"))
}

// ---- Nginx ---------------------------------------------------------------------------

pub struct Nginx;

impl Nginx {
    fn conf(ctx: &ServiceCtx) -> PathBuf {
        ctx.service_dir.join("conf").join("nginx.conf")
    }

    /// `-p <prefix>/ -c <conf>` — the same pair for start, test and quit.
    fn prefix_args(ctx: &ServiceCtx) -> Vec<OsString> {
        vec![
            "-p".into(),
            format!("{}/", ng(&ctx.service_dir)).into(),
            "-c".into(),
            ng(&Self::conf(ctx)).into(),
        ]
    }

    pub fn render(ctx: &ServiceCtx) -> String {
        let sd = &ctx.service_dir;
        format!(
            "# Managed by Devluma — regenerated before every start. Put your own directives in\n\
             # custom.conf (included below, never overwritten) or per-site files in sites/.\n\
             worker_processes 1;\n\
             error_log logs/error.log;\n\
             pid logs/nginx.pid;\n\
             events {{ worker_connections 1024; }}\n\
             http {{\n\
             \x20 include \"{mime}\";\n\
             \x20 default_type application/octet-stream;\n\
             \x20 access_log logs/access.log;\n\
             \x20 sendfile on;\n\
             \x20 client_body_temp_path temp/client_body;\n\
             \x20 proxy_temp_path temp/proxy;\n\
             \x20 fastcgi_temp_path temp/fastcgi;\n\
             \x20 uwsgi_temp_path temp/uwsgi;\n\
             \x20 scgi_temp_path temp/scgi;\n\
             \x20 server {{\n\
             \x20   listen 127.0.0.1:{port};\n\
             \x20   server_name localhost;\n\
             \x20   root \"{html}\";\n\
             \x20   index index.html;\n\
             \x20 }}\n\
             \x20 include \"{custom}\";\n\
             \x20 include \"{sites}/*.conf\";\n\
             }}\n",
            mime = ng(&ctx.install_dir.join("conf").join("mime.types")),
            port = ctx.port,
            html = ng(&sd.join("html")),
            custom = ng(&sd.join("conf").join("custom.conf")),
            sites = ng(&sd.join("sites")),
        )
    }
}

impl ServiceBehavior for Nginx {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join("nginx.exe")
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let sd = &ctx.service_dir;
        for dir in ["conf", "logs", "temp", "html", "sites"] {
            std::fs::create_dir_all(sd.join(dir)).map_err(io_fail("create the nginx folders"))?;
        }
        let write_if_missing = |path: PathBuf, text: &str| -> Result<(), Failure> {
            if !path.exists() {
                std::fs::write(&path, text).map_err(io_fail("write a default nginx file"))?;
            }
            Ok(())
        };
        write_if_missing(
            sd.join("conf").join("custom.conf"),
            "# Your own nginx http-level directives.\n",
        )?;
        write_if_missing(
            sd.join("html").join("index.html"),
            "<!doctype html><title>Devluma</title><p>Nginx is running, managed by Devluma.</p>\n",
        )?;
        std::fs::write(Self::conf(ctx), Self::render(ctx)).map_err(io_fail("write nginx.conf"))
    }

    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        Self::conf(ctx)
    }

    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let mut args = vec!["-t".to_owned()];
        args.extend(
            Self::prefix_args(ctx)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned()),
        );
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = winproc::run_captured(&self.image(ctx), &refs, Duration::from_secs(15)).map_err(
            |e| {
                failure(
                    ErrorCode::ProcessExited,
                    format!("Could not run nginx -t: {e}"),
                )
            },
        )?;
        if out.exit_code == Some(0) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConfigInvalid,
                format!("nginx rejected the configuration: {}", out.stderr.trim()),
            ))
        }
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        Launch {
            exe: self.image(ctx),
            args: Self::prefix_args(ctx),
            cwd: ctx.service_dir.clone(),
            env: Vec::new(),
        }
    }

    fn request_stop(&self, ctx: &ServiceCtx, _pid: u32) -> bool {
        let mut args = vec!["-s".to_owned(), "quit".to_owned()];
        args.extend(
            Self::prefix_args(ctx)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned()),
        );
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        matches!(
            winproc::run_captured(&self.image(ctx), &refs, Duration::from_secs(10)),
            Ok(o) if o.exit_code == Some(0)
        )
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(15)
    }

    fn log_files(&self, ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        vec![
            ("error.log".into(), ctx.log_dir().join("error.log")),
            ("access.log".into(), ctx.log_dir().join("access.log")),
        ]
    }

    fn reload(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let mut args = vec!["-s".to_owned(), "reload".to_owned()];
        args.extend(
            Self::prefix_args(ctx)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned()),
        );
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = winproc::run_captured(&self.image(ctx), &refs, Duration::from_secs(15)).map_err(
            |e| {
                failure(
                    ErrorCode::ProcessExited,
                    format!("Could not run nginx -s reload: {e}"),
                )
            },
        )?;
        if out.exit_code == Some(0) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConfigInvalid,
                format!("nginx refused to reload: {}", out.stderr.trim()),
            ))
        }
    }
}

// ---- Apache httpd --------------------------------------------------------------------

/// Apache httpd (Apache Lounge build) in console mode. Devluma writes the whole `httpd.conf`;
/// the install's own `conf/` is only used for `mime.types`.
pub struct Apache;

/// Modules the generated config needs (all ship with the Apache Lounge build).
const APACHE_MODULES: &[&str] = &[
    "alias",
    "authz_core",
    "authz_host",
    "dir",
    "env",
    "headers",
    "log_config",
    "mime",
    "proxy",
    "proxy_fcgi",
    "rewrite",
    "setenvif",
    "socache_shmcb",
    "ssl",
];

impl Apache {
    fn conf(ctx: &ServiceCtx) -> PathBuf {
        ctx.service_dir.join("conf").join("httpd.conf")
    }

    pub fn render(ctx: &ServiceCtx) -> String {
        let sd = &ctx.service_dir;
        let modules: String = APACHE_MODULES
            .iter()
            .map(|m| format!("LoadModule {m}_module modules/mod_{m}.so\n"))
            .collect();
        format!(
            "# Managed by Devluma — regenerated before every start. Put your own directives in\n\
             # custom.conf (included below, never overwritten) or per-site files in sites/.\n\
             ServerRoot \"{root}\"\n\
             Listen 127.0.0.1:{port}\n\
             {modules}\
             ServerName localhost\n\
             PidFile \"{logs}/httpd.pid\"\n\
             ErrorLog \"{logs}/error.log\"\n\
             LogFormat \"%h %l %u %t \\\"%r\\\" %>s %b\" common\n\
             CustomLog \"{logs}/access.log\" common\n\
             TypesConfig conf/mime.types\n\
             SSLSessionCache \"shmcb:{logs}/ssl_scache(512000)\"\n\
             <Directory />\n  AllowOverride None\n  Require all denied\n</Directory>\n\
             # Default site for requests that match no Devluma site.\n\
             <VirtualHost 127.0.0.1:{port}>\n\
             \x20 DocumentRoot \"{html}\"\n\
             \x20 <Directory \"{html}\">\n    Require all granted\n  </Directory>\n\
             </VirtualHost>\n\
             Include \"{custom}\"\n\
             IncludeOptional \"{sites}/*.conf\"\n",
            root = ng(&ctx.install_dir),
            port = ctx.port,
            logs = ng(&ctx.log_dir()),
            html = ng(&sd.join("html")),
            custom = ng(&sd.join("conf").join("custom.conf")),
            sites = ng(&sd.join("sites")),
        )
    }

    /// Graceful restart/shutdown: mpm_winnt's console-mode parent waits on these events
    /// (verified in review: `ap<pid>_restart` starts a new child, `ap<pid>_shutdown` exits).
    fn signal(pid: u32, what: &str) -> bool {
        winproc::signal_named_event(&format!("ap{pid}_{what}"))
    }

    fn httpd(&self, ctx: &ServiceCtx, extra: &[&str]) -> Result<winproc::Output, Failure> {
        let conf = ng(&Self::conf(ctx));
        let mut args: Vec<&str> = extra.to_vec();
        args.extend(["-f", &conf]);
        winproc::run_captured(&self.image(ctx), &args, Duration::from_secs(15)).map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not run httpd: {e}"),
            )
        })
    }

    fn pid(ctx: &ServiceCtx) -> Option<u32> {
        std::fs::read_to_string(ctx.log_dir().join("httpd.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

impl ServiceBehavior for Apache {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join("bin").join("httpd.exe")
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let sd = &ctx.service_dir;
        for dir in ["conf", "logs", "html", "sites"] {
            std::fs::create_dir_all(sd.join(dir)).map_err(io_fail("create the Apache folders"))?;
        }
        let write_if_missing = |path: PathBuf, text: &str| -> Result<(), Failure> {
            if !path.exists() {
                std::fs::write(&path, text).map_err(io_fail("write a default Apache file"))?;
            }
            Ok(())
        };
        write_if_missing(
            sd.join("conf").join("custom.conf"),
            "# Your own Apache directives.\n",
        )?;
        write_if_missing(
            sd.join("html").join("index.html"),
            "<!doctype html><title>Devluma</title><p>Apache is running, managed by Devluma.</p>\n",
        )?;
        std::fs::write(Self::conf(ctx), Self::render(ctx)).map_err(io_fail("write httpd.conf"))
    }

    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        Self::conf(ctx)
    }

    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let out = self.httpd(ctx, &["-t"])?;
        if out.exit_code == Some(0) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConfigInvalid,
                format!("Apache rejected the configuration: {}", out.stderr.trim()),
            ))
        }
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        Launch {
            exe: self.image(ctx),
            args: vec!["-f".into(), ng(&Self::conf(ctx)).into()],
            cwd: ctx.service_dir.clone(),
            env: Vec::new(),
        }
    }

    fn request_stop(&self, _ctx: &ServiceCtx, pid: u32) -> bool {
        Self::signal(pid, "shutdown")
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(20)
    }

    fn log_files(&self, ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        vec![
            ("error.log".into(), ctx.log_dir().join("error.log")),
            ("access.log".into(), ctx.log_dir().join("access.log")),
        ]
    }

    fn reload(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        self.validate(ctx)?;
        match Self::pid(ctx) {
            Some(pid) if Self::signal(pid, "restart") => Ok(()),
            _ => Err(failure(
                ErrorCode::ProcessExited,
                "Apache didn't accept the reload request; restart the service instead.",
            )),
        }
    }
}

// ---- PHP (FastCGI) -------------------------------------------------------------------

/// `php-cgi -b 127.0.0.1:<port>`: one backend per PHP version used by sites (ADR 0009).
pub struct PhpCgi;

impl ServiceBehavior for PhpCgi {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join("php-cgi.exe")
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        std::fs::create_dir_all(ctx.log_dir()).map_err(io_fail("create the log folder"))
    }

    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join(crate::services::php::INI_FILE)
    }

    fn validate(&self, _ctx: &ServiceCtx) -> Result<(), Failure> {
        Ok(())
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        let mut args: Vec<OsString> = vec!["-b".into(), format!("127.0.0.1:{}", ctx.port).into()];
        // Same php.ini the CLI uses, if one exists; otherwise built-in defaults.
        let ini = self.config_path(ctx);
        if ini.is_file() {
            args.push("-c".into());
            args.push(ini.into_os_string());
        }
        Launch {
            exe: self.image(ctx),
            args,
            cwd: ctx.install_dir.clone(),
            // Default is 500: php-cgi would exit (and look crashed) after 500 requests.
            env: vec![("PHP_FCGI_MAX_REQUESTS".into(), "0".into())],
        }
    }

    fn request_stop(&self, _ctx: &ServiceCtx, _pid: u32) -> bool {
        false // stateless: stopping the owned job is the clean way
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(15)
    }

    fn log_files(&self, _ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        Vec::new()
    }
}

// ---- Mailpit -------------------------------------------------------------------------

/// Local mail catcher: SMTP on the service port, web UI on the admin port. Messages are kept
/// in a database inside the service folder.
pub struct Mailpit;

impl Mailpit {
    fn ui_port(ctx: &ServiceCtx) -> u16 {
        ctx.admin_port.unwrap_or(8025)
    }

    pub fn render_readme(ctx: &ServiceCtx) -> String {
        format!(
            "Mailpit, managed by Devluma. Nothing here is sent to real recipients.\n\n\
             SMTP for your apps:  127.0.0.1:{smtp} (no TLS; any username/password is accepted)\n\
             Web interface:       http://127.0.0.1:{ui}/\n\n\
             Laravel .env example:\n\
             MAIL_MAILER=smtp\n\
             MAIL_HOST=127.0.0.1\n\
             MAIL_PORT={smtp}\n\n\
             PHP's mail(): set `SMTP = 127.0.0.1` and `smtp_port = {smtp}` in php.ini.\n",
            smtp = ctx.port,
            ui = Self::ui_port(ctx),
        )
    }
}

impl ServiceBehavior for Mailpit {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join("mailpit.exe")
    }

    fn admin_port(&self) -> Option<AdminPort> {
        Some(AdminPort {
            label: "web interface",
            default: 8025,
        })
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        for dir in [ctx.log_dir(), ctx.service_dir.join("data")] {
            std::fs::create_dir_all(dir).map_err(io_fail("create the Mailpit folders"))?;
        }
        std::fs::write(self.config_path(ctx), Self::render_readme(ctx))
            .map_err(io_fail("write the Mailpit connection notes"))
    }

    /// Mailpit is configured by arguments only; this file explains how to connect.
    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.service_dir.join("connection.txt")
    }

    fn validate(&self, _ctx: &ServiceCtx) -> Result<(), Failure> {
        Ok(())
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        Launch {
            exe: self.image(ctx),
            args: vec![
                "--smtp".into(),
                format!("127.0.0.1:{}", ctx.port).into(),
                "--listen".into(),
                format!("127.0.0.1:{}", Self::ui_port(ctx)).into(),
                "--database".into(),
                ctx.service_dir
                    .join("data")
                    .join("mailpit.db")
                    .into_os_string(),
                // Apps often send credentials; accept them without TLS (local only).
                "--smtp-auth-accept-any".into(),
                "--smtp-auth-allow-insecure".into(),
                // No background calls to GitHub.
                "--disable-version-check".into(),
            ],
            cwd: ctx.service_dir.clone(),
            env: Vec::new(),
        }
    }

    fn request_stop(&self, _ctx: &ServiceCtx, _pid: u32) -> bool {
        false // no shutdown command; its SQLite database tolerates the job being stopped
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(15)
    }

    fn log_files(&self, _ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        Vec::new()
    }
}

// ---- PostgreSQL ----------------------------------------------------------------------

/// PostgreSQL run directly as `postgres.exe -D <data>`; port and loopback binding are passed
/// on the command line so the user's `postgresql.conf` can't widen them by accident.
pub struct Postgresql;

impl Postgresql {
    fn bin(ctx_install: &Path, exe: &str) -> PathBuf {
        ctx_install.join("bin").join(exe)
    }

    fn data(ctx: &ServiceCtx) -> Result<&Path, Failure> {
        ctx.data_dir.as_deref().ok_or_else(|| {
            failure(
                ErrorCode::ValidationError,
                "This database has no data folder configured.",
            )
        })
    }
}

impl ServiceBehavior for Postgresql {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        Self::bin(&ctx.install_dir, "postgres.exe")
    }

    fn is_data_initialized(&self, dir: &Path) -> bool {
        dir.join("PG_VERSION").is_file()
    }

    /// `initdb` with a passwordless `postgres` superuser, trusted only on this computer.
    fn initialize_data(
        &self,
        install_dir: &Path,
        data_dir: &Path,
        _port: u16,
    ) -> Result<(), Failure> {
        ensure_empty_data_dir(data_dir)?;
        if let Some(parent) = data_dir.parent() {
            std::fs::create_dir_all(parent).map_err(io_fail("create the data folder's parent"))?;
        }
        let pgdata = data_dir.display().to_string();
        let out = winproc::run_captured(
            &Self::bin(install_dir, "initdb.exe"),
            &[
                "-D",
                &pgdata,
                "-U",
                "postgres",
                "--auth-host=trust",
                "--auth-local=trust",
                "--encoding=UTF8",
                "--locale=C",
            ],
            Duration::from_secs(180),
        )
        .map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not run initdb: {e}"),
            )
        })?;
        if out.exit_code != Some(0) || !self.is_data_initialized(data_dir) {
            return Err(failure(
                ErrorCode::ProcessExited,
                format!(
                    "initdb failed (exit code {:?}): {}",
                    out.exit_code,
                    out.stderr.trim().chars().take(500).collect::<String>()
                ),
            ));
        }
        Ok(())
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        std::fs::create_dir_all(ctx.log_dir()).map_err(io_fail("create the log folder"))
    }

    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.data_dir
            .as_ref()
            .map(|d| d.join("postgresql.conf"))
            .unwrap_or_default()
    }

    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let data = Self::data(ctx)?;
        if !self.is_data_initialized(data) {
            return Err(failure(
                ErrorCode::ConfigInvalid,
                format!(
                    "{} is not an initialized PostgreSQL data folder.",
                    data.display()
                ),
            ));
        }
        Ok(())
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        let data = ctx.data_dir.clone().unwrap_or_default();
        Launch {
            exe: self.image(ctx),
            args: vec![
                "-D".into(),
                data.into_os_string(),
                "-p".into(),
                ctx.port.to_string().into(),
                // Local development only: never listen on other interfaces.
                "-c".into(),
                "listen_addresses=127.0.0.1".into(),
            ],
            cwd: ctx.install_dir.clone(),
            env: Vec::new(),
        }
    }

    /// `pg_ctl stop -m fast`: ends sessions and checkpoints, as PostgreSQL documents.
    fn request_stop(&self, ctx: &ServiceCtx, _pid: u32) -> bool {
        let Ok(data) = Self::data(ctx) else {
            return false;
        };
        let pgdata = data.display().to_string();
        matches!(
            winproc::run_captured(
                &Self::bin(&ctx.install_dir, "pg_ctl.exe"),
                &["stop", "-D", &pgdata, "-m", "fast", "-W"],
                Duration::from_secs(10),
            ),
            Ok(o) if o.exit_code == Some(0)
        )
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

    fn log_files(&self, _ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        Vec::new()
    }
}

// ---- MongoDB -------------------------------------------------------------------------

/// `mongod` with access control off, bound to loopback; data in the user's data folder.
pub struct Mongodb;

impl ServiceBehavior for Mongodb {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join("bin").join("mongod.exe")
    }

    fn is_data_initialized(&self, dir: &Path) -> bool {
        dir.join("WiredTiger").is_file()
    }

    /// mongod creates its files on first start; only the (empty) folder is made here.
    fn initialize_data(
        &self,
        _install_dir: &Path,
        data_dir: &Path,
        _port: u16,
    ) -> Result<(), Failure> {
        ensure_empty_data_dir(data_dir)?;
        std::fs::create_dir_all(data_dir).map_err(io_fail("create the data folder"))
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        std::fs::create_dir_all(ctx.log_dir()).map_err(io_fail("create the log folder"))?;
        std::fs::write(self.config_path(ctx), Mongodb::render(ctx))
            .map_err(io_fail("write mongod.conf"))
    }

    /// Generated each start (YAML); port, binding and paths come from Devluma.
    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.service_dir.join("mongod.conf")
    }

    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        match &ctx.data_dir {
            Some(d) if d.is_dir() => Ok(()),
            _ => Err(failure(
                ErrorCode::ConfigInvalid,
                "The MongoDB data folder is missing.",
            )),
        }
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        Launch {
            exe: self.image(ctx),
            args: vec!["--config".into(), self.config_path(ctx).into_os_string()],
            cwd: ctx.service_dir.clone(),
            env: Vec::new(),
        }
    }

    fn request_stop(&self, _ctx: &ServiceCtx, pid: u32) -> bool {
        // mongod on Windows shuts down cleanly when this event is set (verified in review).
        winproc::signal_named_event(&format!("Global\\Mongo_{pid}"))
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

    fn log_files(&self, ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        vec![("mongod.log".into(), ctx.log_dir().join("mongod.log"))]
    }
}

impl Mongodb {
    pub fn render(ctx: &ServiceCtx) -> String {
        // YAML strings in single quotes: backslashes are literal, quotes are doubled.
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
        format!(
            "# Managed by Devluma — regenerated before every start.\n\
             net:\n  bindIp: 127.0.0.1\n  port: {port}\n\
             storage:\n  dbPath: {data}\n\
             systemLog:\n  destination: file\n  path: {log}\n  logAppend: true\n",
            port = ctx.port,
            data = q(ctx.data_dir.as_deref().unwrap_or(Path::new(""))),
            log = q(&ctx.log_dir().join("mongod.log")),
        )
    }
}

// ---- MariaDB -------------------------------------------------------------------------

pub struct Mariadb;

impl Mariadb {
    fn data(ctx: &ServiceCtx) -> Result<&Path, Failure> {
        ctx.data_dir.as_deref().ok_or_else(|| {
            failure(
                ErrorCode::ValidationError,
                "This database has no data folder configured.",
            )
        })
    }

    fn ini(ctx: &ServiceCtx) -> Option<PathBuf> {
        ctx.data_dir.as_ref().map(|d| d.join("my.ini"))
    }

    /// True if `dir` looks like an initialized MariaDB/MySQL data folder.
    pub fn is_initialized(dir: &Path) -> bool {
        dir.join("mysql").is_dir()
    }

    /// Runs `mariadb-install-db` for an empty or missing data folder. Refuses anything else.
    pub fn initialize(install_dir: &Path, data_dir: &Path, port: u16) -> Result<(), Failure> {
        ensure_empty_data_dir(data_dir)?;
        // mariadb-install-db creates the data folder itself but not missing parents.
        if let Some(parent) = data_dir.parent() {
            std::fs::create_dir_all(parent).map_err(io_fail("create the data folder's parent"))?;
        }
        let datadir = format!("--datadir={}", data_dir.display());
        let port = format!("--port={port}");
        let out = winproc::run_captured(
            &install_dir.join("bin").join("mariadb-install-db.exe"),
            &[&datadir, &port],
            Duration::from_secs(180),
        )
        .map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not run mariadb-install-db: {e}"),
            )
        })?;
        if out.exit_code != Some(0) || !Self::is_initialized(data_dir) {
            return Err(failure(
                ErrorCode::ProcessExited,
                format!(
                    "mariadb-install-db failed (exit code {:?}): {}",
                    out.exit_code,
                    out.stderr.trim().chars().take(500).collect::<String>()
                ),
            ));
        }
        Ok(())
    }
}

impl ServiceBehavior for Mariadb {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join("bin").join("mariadbd.exe")
    }

    fn is_data_initialized(&self, dir: &Path) -> bool {
        Self::is_initialized(dir)
    }

    fn initialize_data(
        &self,
        install_dir: &Path,
        data_dir: &Path,
        port: u16,
    ) -> Result<(), Failure> {
        Self::initialize(install_dir, data_dir, port)
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        std::fs::create_dir_all(ctx.log_dir()).map_err(io_fail("create the log folder"))?;
        Ok(())
    }

    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        Self::ini(ctx).unwrap_or_default()
    }

    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let data = Self::data(ctx)?;
        if !Self::is_initialized(data) {
            return Err(failure(
                ErrorCode::ConfigInvalid,
                format!(
                    "{} is not an initialized MariaDB data folder.",
                    data.display()
                ),
            ));
        }
        if !data.join("my.ini").is_file() {
            return Err(failure(
                ErrorCode::ConfigInvalid,
                format!("{} has no my.ini.", data.display()),
            ));
        }
        Ok(())
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        let ini = Self::ini(ctx).unwrap_or_default();
        Launch {
            exe: self.image(ctx),
            args: vec![
                format!("--defaults-file={}", ini.display()).into(),
                // my.ini (written by mariadb-install-db) holds an absolute datadir; passing it
                // here keeps a moved Devluma folder working (ADR 0014).
                format!(
                    "--datadir={}",
                    ctx.data_dir.as_deref().unwrap_or(Path::new("")).display()
                )
                .into(),
                format!("--port={}", ctx.port).into(),
                // Local development only: never listen on other interfaces.
                "--bind-address=127.0.0.1".into(),
                "--console".into(),
            ],
            cwd: ctx.install_dir.clone(),
            env: Vec::new(),
        }
    }

    fn request_stop(&self, _ctx: &ServiceCtx, pid: u32) -> bool {
        // mariadbd on Windows waits on this event for a clean shutdown (verified in review).
        winproc::signal_named_event(&format!("MySQLShutdown{pid}"))
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

    fn log_files(&self, _ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    //! A fake service implemented by re-running the test binary (see
    //! `supervisor::tests::fake_service`). Behaviour is chosen with `DEVLUMA_FAKE_MODE`.

    use super::*;

    pub struct Fixture;

    /// Fake behaviour per service id (tests run in parallel).
    pub static MODES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

    pub fn set_mode(service_id: &str, mode: &str) {
        let mut m = MODES.lock().unwrap();
        m.retain(|(id, _)| id != service_id);
        m.push((service_id.to_owned(), mode.to_owned()));
    }

    fn mode(service_id: &str) -> String {
        MODES
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| id == service_id)
            .map_or_else(|| "serve".to_owned(), |(_, m)| m.clone())
    }

    impl ServiceBehavior for Fixture {
        fn image(&self, _ctx: &ServiceCtx) -> PathBuf {
            std::env::current_exe().unwrap()
        }
        fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
            std::fs::create_dir_all(ctx.log_dir()).map_err(io_fail("create logs"))?;
            std::fs::write(
                ctx.service_dir.join("fake.conf"),
                format!("port={}\n", ctx.port),
            )
            .map_err(io_fail("write config"))
        }
        fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
            ctx.service_dir.join("fake.conf")
        }
        fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
            match mode(&ctx.service_id).as_str() {
                "invalid_config" => {
                    Err(failure(ErrorCode::ConfigInvalid, "fake: bad config line 3"))
                }
                _ => Ok(()),
            }
        }
        fn launch(&self, ctx: &ServiceCtx) -> Launch {
            Launch {
                exe: self.image(ctx),
                args: vec![
                    "--exact".into(),
                    "services::supervisor::tests::fake_service".into(),
                    "--ignored".into(),
                    "--nocapture".into(),
                ],
                cwd: ctx.service_dir.clone(),
                env: vec![
                    ("DEVLUMA_FAKE_PORT".into(), ctx.port.to_string()),
                    ("DEVLUMA_FAKE_MODE".into(), mode(&ctx.service_id)),
                    // Dies with the test process even if the run aborts.
                    ("DEVLUMA_TEST_PARENT".into(), std::process::id().to_string()),
                ],
            }
        }
        fn request_stop(&self, _ctx: &ServiceCtx, pid: u32) -> bool {
            winproc::signal_named_event(&format!("DevlumaFakeStop{pid}"))
        }
        fn ready_timeout(&self) -> Duration {
            Duration::from_secs(10)
        }
        fn log_files(&self, _ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Path) -> ServiceCtx {
        ServiceCtx {
            service_id: "s1".into(),
            install_dir: PathBuf::from(r"C:\Devluma\packages\nginx\1.30.5-x64"),
            service_dir: dir.to_path_buf(),
            data_dir: None,
            port: 8080,
            admin_port: None,
        }
    }

    #[test]
    fn nginx_config_binds_loopback_and_keeps_user_files() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        Nginx.prepare(&c).unwrap();
        let conf = std::fs::read_to_string(Nginx.config_path(&c)).unwrap();
        assert!(conf.contains("listen 127.0.0.1:8080;"));
        assert!(conf.contains("C:/Devluma/packages/nginx/1.30.5-x64/conf/mime.types"));
        assert!(!conf.contains('\\'), "nginx paths must use forward slashes");

        std::fs::write(dir.path().join("conf").join("custom.conf"), "gzip on;\n").unwrap();
        Nginx.prepare(&c).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("conf").join("custom.conf")).unwrap(),
            "gzip on;\n",
            "user-owned custom.conf is never overwritten"
        );
    }

    #[test]
    fn mariadb_refuses_to_initialize_over_existing_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ibdata1"), b"someone's data").unwrap();
        let err = Mariadb::initialize(Path::new(r"C:\nowhere"), dir.path(), 3306).unwrap_err();
        assert_eq!(err.code, ErrorCode::DataDirectoryExists);
        assert_eq!(
            std::fs::read(dir.path().join("ibdata1")).unwrap(),
            b"someone's data"
        );
    }

    #[test]
    fn mariadb_launch_is_loopback_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.data_dir = Some(dir.path().join("data"));
        let args: Vec<String> = Mariadb
            .launch(&c)
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--bind-address=127.0.0.1".to_owned()));
        assert!(args.contains(&"--port=8080".to_owned()));
        assert!(
            Mariadb.validate(&c).is_err(),
            "uninitialized data folder is rejected"
        );
    }
}
