//! Per-adapter service behaviour: config generation, native validation, launch command,
//! graceful stop and log locations. Everything here is compiled code — catalog data never
//! contributes commands or arguments.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::domain::adapter::AdapterId;
use crate::domain::config_settings;
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
    /// Simple settings for generated configs (`service_settings`, canonical values).
    pub settings: BTreeMap<String, String>,
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
    /// Files shown in the advanced editor (ADR 0018). The first is the main one.
    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![ConfigFile::generated(
            "main",
            "Configuration",
            self.config_path(ctx),
            "text",
        )]
    }
    /// The check run after a config file is saved: the native validator, plus a parse of the
    /// tool's own file where `validate` doesn't read it. Returns warnings that didn't block.
    fn check_config(&self, ctx: &ServiceCtx) -> Result<Vec<String>, Failure> {
        self.validate(ctx).map(|()| Vec::new())
    }
    /// What the simple settings generate, for the preview diff: the generated config, or the
    /// launch options for tools configured by arguments. `None`: settings live in a file.
    fn render_config(&self, _ctx: &ServiceCtx) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminPort {
    pub label: &'static str,
    pub default: u16,
    /// A page a browser can open (Mailpit's UI), not an API (Caddy's admin endpoint).
    pub web_interface: bool,
}

/// A config file the advanced editor shows.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub key: &'static str,
    pub label: &'static str,
    pub path: PathBuf,
    /// False for files Devluma regenerates (editing them would be lost on the next start).
    pub editable: bool,
    /// Editor highlighting: `ini`, `nginx`, `apache`, `caddyfile`, `yaml` or `text`.
    pub language: &'static str,
    pub note: &'static str,
}

impl ConfigFile {
    fn editable(
        key: &'static str,
        label: &'static str,
        path: PathBuf,
        language: &'static str,
        note: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            path,
            editable: true,
            language,
            note,
        }
    }

    fn generated(
        key: &'static str,
        label: &'static str,
        path: PathBuf,
        language: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            path,
            editable: false,
            language,
            note: "Devluma writes this file before every start, so changes here would be lost. Use the Settings tab or the editable file instead.",
        }
    }
}

/// Simple settings for `adapter`, re-validated and written in the tool's syntax. Values that
/// fail validation are skipped, so a bad row can never inject a directive.
pub fn managed_settings(ctx: &ServiceCtx, adapter: AdapterId) -> Vec<(&'static str, String)> {
    let Some(schema) = config_settings::schema(adapter) else {
        return Vec::new();
    };
    schema
        .specs
        .iter()
        .filter_map(|spec| {
            let value = config_settings::validate(spec, ctx.settings.get(spec.key)?).ok()?;
            Some((
                spec.key,
                config_settings::encode(schema.syntax, spec, &value),
            ))
        })
        .collect()
}

fn setting(ctx: &ServiceCtx, adapter: AdapterId, key: &str) -> Option<String> {
    managed_settings(ctx, adapter)
        .into_iter()
        .find_map(|(k, v)| (k == key).then_some(v))
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
        AdapterId::Caddy => Some(&CADDY),
        AdapterId::Frankenphp => Some(&FRANKENPHP),
        AdapterId::Node | AdapterId::Python | AdapterId::Cmder => None,
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
        let worker_connections =
            setting(ctx, AdapterId::Nginx, "worker_connections").unwrap_or_else(|| "1024".into());
        // Before `include custom.conf`, so nginx reports a duplicate in custom.conf's line.
        let settings: String = managed_settings(ctx, AdapterId::Nginx)
            .into_iter()
            .filter(|(k, _)| *k != "worker_connections")
            .map(|(k, v)| format!("  {k} {v};\n"))
            .collect();
        format!(
            "# Managed by Devluma — regenerated before every start. Put your own directives in\n\
             # custom.conf (included below, never overwritten) or per-site files in sites/.\n\
             worker_processes 1;\n\
             error_log logs/error.log;\n\
             pid logs/nginx.pid;\n\
             events {{ worker_connections {worker_connections}; }}\n\
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
             {settings}\
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

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![
            ConfigFile::editable(
                "custom",
                "custom.conf",
                ctx.service_dir.join("conf").join("custom.conf"),
                "nginx",
                "Your own directives, included inside http { }. nginx -t checks the whole configuration when you save.",
            ),
            ConfigFile::generated("main", "nginx.conf", Self::conf(ctx), "nginx"),
        ]
    }

    fn render_config(&self, ctx: &ServiceCtx) -> Option<String> {
        Some(Self::render(ctx))
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
        // Before `Include custom.conf`: a later directive wins, so custom.conf can override.
        let settings: String = managed_settings(ctx, AdapterId::Apache)
            .into_iter()
            .map(|(k, v)| format!("{k} {v}\n"))
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
             {settings}\
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

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![
            ConfigFile::editable(
                "custom",
                "custom.conf",
                ctx.service_dir.join("conf").join("custom.conf"),
                "apache",
                "Your own directives, included at the end of httpd.conf (they override the Settings tab). httpd -t checks the whole configuration when you save.",
            ),
            ConfigFile::generated("main", "httpd.conf", Self::conf(ctx), "apache"),
        ]
    }

    fn render_config(&self, ctx: &ServiceCtx) -> Option<String> {
        Some(Self::render(ctx))
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

// ---- Caddy and FrankenPHP --------------------------------------------------------------

/// Caddy 2 with a Devluma-written Caddyfile (ADR 0019): automatic HTTPS off (sites use
/// certificates from Devluma's local CA), never installs a CA into a trust store, data and
/// config folders inside the service folder, and the admin API on a loopback admin port —
/// used for graceful reload and stop.
///
/// FrankenPHP is the same server with PHP built in: the same Caddyfile, plus `php_server`
/// for PHP sites and a php.ini of its own in the service folder (found through `PHPRC`).
/// Its executable is the user's own copy (ADR 0019, `services::frankenphp`).
pub struct Caddy {
    frankenphp: bool,
}

pub static CADDY: Caddy = Caddy { frankenphp: false };
pub static FRANKENPHP: Caddy = Caddy { frankenphp: true };

/// Caddy's own default admin port (FrankenPHP's too).
pub const CADDY_ADMIN_PORT: u16 = 2019;

/// Extensions a new FrankenPHP php.ini enables, when the build ships them: what Laravel,
/// Symfony and WordPress commonly need.
const FRANKENPHP_EXTENSIONS: &[&str] = &[
    "curl",
    "fileinfo",
    "gd",
    "intl",
    "mbstring",
    "mysqli",
    "openssl",
    "pdo_mysql",
    "pdo_pgsql",
    "pdo_sqlite",
    "sqlite3",
    "zip",
];

impl Caddy {
    fn name(&self) -> &'static str {
        if self.frankenphp {
            "FrankenPHP"
        } else {
            "Caddy"
        }
    }

    fn adapter(&self) -> AdapterId {
        if self.frankenphp {
            AdapterId::Frankenphp
        } else {
            AdapterId::Caddy
        }
    }

    fn conf(ctx: &ServiceCtx) -> PathBuf {
        ctx.service_dir.join("conf").join("Caddyfile")
    }

    /// FrankenPHP's php.ini (the folder is passed as `PHPRC`).
    pub fn php_ini(ctx: &ServiceCtx) -> PathBuf {
        ctx.service_dir
            .join("php")
            .join(crate::services::php::INI_FILE)
    }

    fn admin_address(ctx: &ServiceCtx) -> String {
        format!("127.0.0.1:{}", ctx.admin_port.unwrap_or(CADDY_ADMIN_PORT))
    }

    /// Caddy keeps certificates/locks in its data folder and autosaves into its config
    /// folder; both default to `%AppData%\Caddy`. Every invocation points them here instead.
    /// FrankenPHP's PHP reads php.ini from `PHPRC`, never from the user's own folder.
    fn env(&self, ctx: &ServiceCtx) -> Vec<(String, String)> {
        let mut env = vec![
            (
                "XDG_DATA_HOME".into(),
                ctx.service_dir.join("data").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                ctx.service_dir.join("config").display().to_string(),
            ),
        ];
        if self.frankenphp {
            env.push((
                "PHPRC".into(),
                ctx.service_dir.join("php").display().to_string(),
            ));
        }
        env
    }

    /// `--config <Caddyfile> --adapter caddyfile`, shared by run, validate and reload.
    fn config_args(ctx: &ServiceCtx) -> Vec<String> {
        vec![
            "--config".into(),
            ng(&Self::conf(ctx)),
            "--adapter".into(),
            "caddyfile".into(),
        ]
    }

    fn cli(&self, ctx: &ServiceCtx, args: &[String]) -> Result<winproc::Output, Failure> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        winproc::run_captured_env(
            &self.image(ctx),
            &refs,
            &self.env(ctx),
            Duration::from_secs(20),
        )
        .map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!(
                    "Could not run {} {}: {e}",
                    self.name().to_lowercase(),
                    args.first().map_or("", |a| a)
                ),
            )
        })
    }

    /// Caddy's error, without its JSON log lines (it logs to stderr while validating).
    fn error_text(out: &winproc::Output) -> String {
        let text = format!("{}\n{}", out.stderr, out.stdout);
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('{'))
            .collect();
        let msg = if lines.is_empty() {
            text.trim().to_owned()
        } else {
            lines.join(" ")
        };
        msg.chars().take(800).collect()
    }

    /// The Settings tab's values: server timeouts (global options) and the two snippets every
    /// Devluma site imports (`devluma_settings`, and `devluma_php` for PHP), so a change
    /// applies to all sites without rewriting their files. Caddy's `devluma_php <backend>` is
    /// `php_fastcgi`; FrankenPHP's `devluma_php` is `php_server`, its built-in PHP.
    fn render_settings(&self, ctx: &ServiceCtx) -> (String, String) {
        let get = |key| setting(ctx, self.adapter(), key);
        let servers = get("idle_timeout").map_or_else(String::new, |s| {
            format!("\tservers {{\n\t\ttimeouts {{\n\t\t\tidle {s}s\n\t\t}}\n\t}}\n")
        });
        let mut site = String::new();
        if let Some(max) = get("max_size").filter(|m| m != "0") {
            site.push_str(&format!("\trequest_body {{\n\t\tmax_size {max}\n\t}}\n"));
        }
        if get("encode").as_deref() == Some("on") {
            site.push_str("\tencode zstd gzip\n");
        }
        if site.is_empty() {
            site.push_str("\t# Nothing changed on the Settings tab.\n");
        }
        let php = if self.frankenphp {
            "\tphp_server\n".to_owned()
        } else {
            match get("php_read_timeout").filter(|t| t != "0") {
                Some(t) => {
                    format!("\tphp_fastcgi {{args[0]}} {{\n\t\tread_timeout {t}s\n\t}}\n")
                }
                None => "\tphp_fastcgi {args[0]}\n".into(),
            }
        };
        let snippets = format!(
            "# Settings tab values, imported by every Devluma site.\n\
             (devluma_settings) {{\n{site}}}\n\
             (devluma_php) {{\n{php}}}\n"
        );
        (servers, snippets)
    }

    pub fn render(&self, ctx: &ServiceCtx) -> String {
        let sd = &ctx.service_dir;
        let (servers, snippets) = self.render_settings(ctx);
        let name = self.name();
        // Caddyfile style is tabs; `caddy validate` warns about anything else.
        format!(
            "# Managed by Devluma — regenerated before every start. Put your own site blocks or\n\
             # snippets in custom.caddy (imported below, never overwritten); per-site files are in sites/.\n\
             {{\n\
             \tadmin {admin}\n\
             \tpersist_config off\n\
             \t# Sites use certificates from Devluma's local CA (ADR 0010); {name} never issues or\n\
             \t# trusts certificates by itself.\n\
             \tauto_https off\n\
             \tskip_install_trust\n\
             \tdefault_bind 127.0.0.1\n\
             \tstorage file_system \"{data}\"\n\
             \tgrace_period 5s\n\
             {servers}\
             }}\n\
             \n\
             {snippets}\
             \n\
             # Default site for requests that match no Devluma site.\n\
             http://:{port} {{\n\
             \timport devluma_settings\n\
             \troot * \"{html}\"\n\
             \tfile_server\n\
             \tlog {{\n\
             \t\toutput file \"{logs}/access.log\"\n\
             \t}}\n\
             }}\n\
             \n\
             import \"{custom}\"\n\
             import \"{sites}/*.conf\"\n",
            admin = Self::admin_address(ctx),
            data = ng(&sd.join("data")),
            port = ctx.port,
            html = ng(&sd.join("html")),
            logs = ng(&ctx.log_dir()),
            custom = ng(&sd.join("conf").join("custom.caddy")),
            sites = ng(&sd.join("sites")),
        )
    }

    /// A first php.ini for FrankenPHP: the build's own development template, an absolute
    /// `extension_dir` (PHP's default is `C:\php\ext`) and common extensions it ships.
    pub fn render_php_ini(install_dir: &Path) -> Result<String, Failure> {
        let template = std::fs::read_to_string(install_dir.join("php.ini-development"))
            .map_err(|_| {
                failure(
                    ErrorCode::ConfigInvalid,
                    "This FrankenPHP folder has no php.ini-development; choose the folder from FrankenPHP's Windows zip.",
                )
            })?;
        let mut text = crate::services::php::render_ini(&template, install_dir);
        for ext in FRANKENPHP_EXTENSIONS {
            if install_dir
                .join("ext")
                .join(format!("php_{ext}.dll"))
                .is_file()
            {
                text = text.replacen(
                    &format!(";extension={ext}\n"),
                    &format!("extension={ext}\n"),
                    1,
                );
                text = text.replacen(
                    &format!(";extension={ext}\r\n"),
                    &format!("extension={ext}\r\n"),
                    1,
                );
            }
        }
        Ok(text)
    }

    /// PHP's own startup warnings for FrankenPHP's php.ini (`php.exe -c <ini> -m`, the CLI of
    /// the same build).
    fn php_ini_problems(ctx: &ServiceCtx) -> Result<Vec<String>, Failure> {
        crate::services::php::ini_problems(
            &crate::services::install::ProcessProbe,
            &ctx.install_dir,
            &Self::php_ini(ctx),
        )
        .map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not check php.ini: {e}"),
            )
        })
    }
}

impl ServiceBehavior for Caddy {
    fn image(&self, ctx: &ServiceCtx) -> PathBuf {
        ctx.install_dir.join(if self.frankenphp {
            "frankenphp.exe"
        } else {
            "caddy.exe"
        })
    }

    fn admin_port(&self) -> Option<AdminPort> {
        Some(AdminPort {
            label: "admin API",
            default: CADDY_ADMIN_PORT,
            web_interface: false,
        })
    }

    fn prepare(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let sd = &ctx.service_dir;
        for dir in ["conf", "logs", "html", "sites", "data", "config"] {
            std::fs::create_dir_all(sd.join(dir)).map_err(io_fail("create the service folders"))?;
        }
        let write_if_missing = |path: PathBuf, text: &str| -> Result<(), Failure> {
            if !path.exists() {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(io_fail("create a folder"))?;
                }
                std::fs::write(&path, text).map_err(io_fail("write a default file"))?;
            }
            Ok(())
        };
        write_if_missing(
            sd.join("conf").join("custom.caddy"),
            "# Your own Caddy site blocks and snippets. Global options can't go here.\n",
        )?;
        write_if_missing(
            sd.join("html").join("index.html"),
            &format!(
                "<!doctype html><title>Devluma</title><p>{} is running, managed by Devluma.</p>\n",
                self.name()
            ),
        )?;
        if self.frankenphp && !Self::php_ini(ctx).exists() {
            write_if_missing(Self::php_ini(ctx), &Self::render_php_ini(&ctx.install_dir)?)?;
        }
        std::fs::write(Self::conf(ctx), self.render(ctx)).map_err(io_fail("write the Caddyfile"))
    }

    fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
        Self::conf(ctx)
    }

    /// `caddy validate` adapts the Caddyfile and provisions every module (including loading
    /// site certificates) without starting servers, so it works while Caddy runs.
    fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        let mut args = vec!["validate".to_owned()];
        args.extend(Self::config_args(ctx));
        let out = self.cli(ctx, &args)?;
        if out.exit_code == Some(0) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConfigInvalid,
                format!(
                    "{} rejected the configuration: {}",
                    self.name(),
                    Self::error_text(&out)
                ),
            ))
        }
    }

    /// FrankenPHP also reports PHP's startup warnings for its php.ini (not blocking: PHP
    /// still starts with them).
    fn check_config(&self, ctx: &ServiceCtx) -> Result<Vec<String>, Failure> {
        self.validate(ctx)?;
        if self.frankenphp {
            Self::php_ini_problems(ctx)
        } else {
            Ok(Vec::new())
        }
    }

    fn launch(&self, ctx: &ServiceCtx) -> Launch {
        let mut args: Vec<OsString> = vec!["run".into()];
        args.extend(Self::config_args(ctx).into_iter().map(OsString::from));
        Launch {
            exe: self.image(ctx),
            args,
            cwd: ctx.service_dir.clone(),
            env: self.env(ctx),
        }
    }

    /// `caddy stop` posts to the admin API's `/stop`; Caddy finishes open requests (up to the
    /// 5 s grace period) and exits.
    fn request_stop(&self, ctx: &ServiceCtx, _pid: u32) -> bool {
        let args = [
            "stop".to_owned(),
            "--address".to_owned(),
            Self::admin_address(ctx),
        ];
        matches!(self.cli(ctx, &args), Ok(o) if o.exit_code == Some(0))
    }

    fn ready_timeout(&self) -> Duration {
        Duration::from_secs(if self.frankenphp { 30 } else { 15 })
    }

    fn log_files(&self, ctx: &ServiceCtx) -> Vec<(String, PathBuf)> {
        vec![("access.log".into(), ctx.log_dir().join("access.log"))]
    }

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        let mut files = vec![ConfigFile::editable(
            "custom",
            "custom.caddy",
            ctx.service_dir.join("conf").join("custom.caddy"),
            "caddyfile",
            "Your own site blocks and snippets, imported by the Caddyfile. The server's own validate command checks the whole configuration when you save.",
        )];
        if self.frankenphp {
            files.push(ConfigFile::editable(
                "php",
                "php.ini",
                Self::php_ini(ctx),
                "ini",
                "PHP settings for this FrankenPHP service only (projects' PHP pins don't apply to it). Restart the service after saving.",
            ));
        }
        files.push(ConfigFile::generated(
            "main",
            "Caddyfile",
            Self::conf(ctx),
            "caddyfile",
        ));
        files
    }

    fn render_config(&self, ctx: &ServiceCtx) -> Option<String> {
        Some(self.render(ctx))
    }

    /// Validate, then `caddy reload`: the admin API swaps configs gracefully and keeps the old
    /// one if the new one fails. `--force` because a changed certificate file has the same
    /// path, so the adapted config would look unchanged.
    fn reload(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
        self.validate(ctx)?;
        let mut args = vec!["reload".to_owned()];
        args.extend(Self::config_args(ctx));
        args.extend([
            "--address".to_owned(),
            Self::admin_address(ctx),
            "--force".to_owned(),
        ]);
        let out = self.cli(ctx, &args)?;
        if out.exit_code == Some(0) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConfigInvalid,
                format!(
                    "{} refused to reload: {}",
                    self.name(),
                    Self::error_text(&out)
                ),
            ))
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

    /// php.ini belongs to the PHP version (shared with the CLI); it's edited on its page.
    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![ConfigFile {
            note: "This backend uses its PHP version's php.ini. Edit it on the PHP version's page, then restart the backend.",
            ..ConfigFile::generated("main", "php.ini", self.config_path(ctx), "ini")
        }]
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
            web_interface: true,
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
        let mut args: Vec<OsString> = vec![
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
        ];
        if let Some(max) = setting(ctx, AdapterId::Mailpit, "max") {
            args.push(format!("--max={max}").into());
        }
        Launch {
            exe: self.image(ctx),
            args,
            cwd: ctx.service_dir.clone(),
            env: Vec::new(),
        }
    }

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![ConfigFile {
            note: "Mailpit is configured with command-line options, set from the Settings tab. This file only explains how to connect.",
            ..ConfigFile::generated("main", "connection.txt", self.config_path(ctx), "text")
        }]
    }

    fn render_config(&self, ctx: &ServiceCtx) -> Option<String> {
        let args: Vec<String> = self
            .launch(ctx)
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        Some(format!("mailpit.exe\n{}\n", args.join("\n")))
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

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![ConfigFile::editable(
            "main",
            "postgresql.conf",
            self.config_path(ctx),
            "ini",
            "In the data folder. Devluma sets the port and listen_addresses at start; those override this file.",
        )]
    }

    /// `postgres -D <data> -C max_connections` reads postgresql.conf (and its includes) and
    /// exits 1 with LOG/HINT lines on an unknown parameter, a bad value or a syntax error.
    /// It works while the server runs (verified with 18.6).
    fn check_config(&self, ctx: &ServiceCtx) -> Result<Vec<String>, Failure> {
        self.validate(ctx)?;
        let pgdata = Self::data(ctx)?.display().to_string();
        let out = winproc::run_captured(
            &self.image(ctx),
            &["-D", &pgdata, "-C", "max_connections"],
            Duration::from_secs(30),
        )
        .map_err(|e| {
            failure(
                ErrorCode::ProcessExited,
                format!("Could not run postgres: {e}"),
            )
        })?;
        if out.exit_code == Some(0) {
            return Ok(Vec::new());
        }
        let reasons: Vec<&str> = out
            .stderr
            .lines()
            .filter_map(|l| {
                ["LOG:", "HINT:", "FATAL:"]
                    .iter()
                    .find_map(|tag| l.split_once(tag).map(|(_, rest)| rest.trim()))
            })
            .filter(|l| !l.starts_with("configuration file"))
            .collect();
        Err(failure(
            ErrorCode::ConfigInvalid,
            format!(
                "PostgreSQL rejected the configuration: {}",
                if reasons.is_empty() {
                    out.stderr.trim().to_owned()
                } else {
                    reasons.join(" ")
                }
            ),
        ))
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

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![ConfigFile::generated(
            "main",
            "mongod.conf",
            self.config_path(ctx),
            "yaml",
        )]
    }

    fn render_config(&self, ctx: &ServiceCtx) -> Option<String> {
        Some(Self::render(ctx))
    }
}

impl Mongodb {
    pub fn render(ctx: &ServiceCtx) -> String {
        // YAML strings in single quotes: backslashes are literal, quotes are doubled.
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
        let get = |key| setting(ctx, AdapterId::Mongodb, key);
        let cache = get("cache_size").map_or_else(String::new, |gb| {
            format!("  wiredTiger:\n    engineConfig:\n      cacheSizeGB: {gb}\n")
        });
        let verbosity =
            get("verbosity").map_or_else(String::new, |v| format!("  verbosity: {v}\n"));
        let profiling = get("slow_op_threshold_ms").map_or_else(String::new, |ms| {
            format!("operationProfiling:\n  slowOpThresholdMs: {ms}\n")
        });
        format!(
            "# Managed by Devluma — regenerated before every start.\n\
             net:\n  bindIp: 127.0.0.1\n  port: {port}\n\
             storage:\n  dbPath: {data}\n{cache}\
             systemLog:\n  destination: file\n  path: {log}\n  logAppend: true\n{verbosity}\
             {profiling}",
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

    fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
        vec![ConfigFile::editable(
            "main",
            "my.ini",
            self.config_path(ctx),
            "ini",
            "In the data folder. Devluma sets the port, data folder and bind address at start; those override this file.",
        )]
    }

    /// `mariadbd <launch options> --help --verbose` parses my.ini like a real start and exits
    /// non-zero on an unknown option, a bad value or a syntax error, with `[ERROR]` lines
    /// (verified with 11.8.9). Out-of-range values are adjusted with a `[Warning]` instead.
    fn check_config(&self, ctx: &ServiceCtx) -> Result<Vec<String>, Failure> {
        self.validate(ctx)?;
        let mut args: Vec<String> = self
            .launch(ctx)
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        args.extend(["--help".into(), "--verbose".into()]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = winproc::run_captured(&self.image(ctx), &refs, Duration::from_secs(30)).map_err(
            |e| {
                failure(
                    ErrorCode::ProcessExited,
                    format!("Could not run mariadbd: {e}"),
                )
            },
        )?;
        let text = format!("{}\n{}", out.stderr, out.stdout);
        let (errors, warnings) = mariadb_messages(&text);
        if out.exit_code == Some(0) {
            return Ok(warnings);
        }
        Err(failure(
            ErrorCode::ConfigInvalid,
            format!(
                "MariaDB rejected the configuration: {}",
                if errors.is_empty() {
                    out.stderr.trim().chars().take(500).collect()
                } else {
                    errors.join(" ")
                }
            ),
        ))
    }
}

/// `[ERROR]` / `[Warning]` log lines (and option-file errors) from mariadbd's output, without
/// timestamps. The help text itself also contains the word "error", so only log lines count.
fn mariadb_messages(text: &str) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for line in text.lines().map(str::trim) {
        let tidy = |s: &str| {
            let s = s.trim().trim_start_matches("Buffered error:").trim();
            // "C:\…\bin\mariadbd.exe: unknown variable 'x=1'" → "unknown variable 'x=1'".
            s.split_once("mariadbd.exe: ")
                .map_or(s, |(_, rest)| rest)
                .to_owned()
        };
        if let Some((_, rest)) = line.split_once("[ERROR]") {
            let msg = tidy(rest);
            if msg != "Aborting" && !errors.contains(&msg) {
                errors.push(msg);
            }
        } else if let Some((_, rest)) = line.split_once("[Warning]") {
            let msg = tidy(rest.trim().trim_start_matches("Buffered warning:"));
            if !warnings.contains(&msg) {
                warnings.push(msg);
            }
        } else if line.starts_with("error:") || line.starts_with("Fatal error in defaults") {
            errors.push(line.to_owned());
        }
    }
    (errors, warnings)
}

#[cfg(test)]
pub(crate) mod tests_support {
    //! A fake service implemented by re-running the test binary (see
    //! `supervisor::tests::fake_service`). Behaviour is chosen with `DEVLUMA_FAKE_MODE`.

    use super::*;

    pub struct Fixture;

    fn render_fixture(ctx: &ServiceCtx) -> String {
        let limit = setting(ctx, AdapterId::Fixture, "limit")
            .map_or_else(String::new, |l| format!("limit={l}\n"));
        format!("port={}\n{limit}", ctx.port)
    }

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
            std::fs::write(ctx.service_dir.join("fake.conf"), render_fixture(ctx))
                .map_err(io_fail("write config"))
        }
        fn config_path(&self, ctx: &ServiceCtx) -> PathBuf {
            ctx.service_dir.join("fake.conf")
        }
        fn validate(&self, ctx: &ServiceCtx) -> Result<(), Failure> {
            let custom =
                std::fs::read_to_string(ctx.service_dir.join("custom.conf")).unwrap_or_default();
            match mode(&ctx.service_id).as_str() {
                "invalid_config" => {
                    Err(failure(ErrorCode::ConfigInvalid, "fake: bad config line 3"))
                }
                _ if custom.contains("invalid") => Err(failure(
                    ErrorCode::ConfigInvalid,
                    "fake: custom.conf line 1 is invalid",
                )),
                _ => Ok(()),
            }
        }
        fn config_files(&self, ctx: &ServiceCtx) -> Vec<ConfigFile> {
            vec![
                ConfigFile::editable(
                    "custom",
                    "custom.conf",
                    ctx.service_dir.join("custom.conf"),
                    "text",
                    "Fake user file.",
                ),
                ConfigFile::generated("main", "fake.conf", self.config_path(ctx), "text"),
            ]
        }
        fn render_config(&self, ctx: &ServiceCtx) -> Option<String> {
            Some(render_fixture(ctx))
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
            settings: BTreeMap::new(),
        }
    }

    #[test]
    fn simple_settings_are_rendered_and_bad_values_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        let default_conf = Nginx::render(&c);
        c.settings = [
            ("client_max_body_size", "64"),
            ("gzip", "on"),
            ("worker_connections", "2048"),
            // Never rendered: fails validation, and isn't a known key.
            ("keepalive_timeout", "5; include C:/evil"),
            ("autoindex", "on"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let conf = Nginx::render(&c);
        assert!(conf.contains("events { worker_connections 2048; }"));
        assert!(conf.contains("  client_max_body_size 64m;\n  gzip on;\n  server {"));
        assert!(!conf.contains("evil") && !conf.contains("autoindex"));
        assert!(
            conf.find("client_max_body_size").unwrap() < conf.find("custom.conf\"").unwrap(),
            "settings come before custom.conf, so nginx names custom.conf in a duplicate"
        );
        assert!(
            !default_conf.contains("client_max_body_size"),
            "defaults aren't written"
        );

        c.settings = [("LimitRequestBody".to_owned(), "unlimited".to_owned())].into();
        let httpd = Apache::render(&c);
        assert!(httpd.contains("LimitRequestBody 0\n"));
        assert!(httpd.find("LimitRequestBody").unwrap() < httpd.find("Include ").unwrap());

        c.settings = [("max".to_owned(), "2000".to_owned())].into();
        let args: Vec<String> = Mailpit
            .launch(&c)
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--max=2000".to_owned()));

        c.settings = [
            ("cache_size".to_owned(), "1536".to_owned()),
            ("verbosity".to_owned(), "1".to_owned()),
        ]
        .into();
        let yaml = Mongodb::render(&c);
        assert!(yaml.contains(
            "storage:\n  dbPath: ''\n  wiredTiger:\n    engineConfig:\n      cacheSizeGB: 1.5\nsystemLog:"
        ));
        assert!(yaml.ends_with("logAppend: true\n  verbosity: 1\n"));

        // Caddy: defaults render no limits; set values land in the global block and snippets.
        c.settings = BTreeMap::new();
        let plain = CADDY.render(&c);
        assert!(plain.contains("(devluma_php) {\n\tphp_fastcgi {args[0]}\n}\n"));
        assert!(!plain.contains("request_body") && !plain.contains("servers {"));
        assert!(!plain.contains("encode "));
        c.settings = [
            ("max_size", "64"),
            ("php_read_timeout", "300"),
            ("idle_timeout", "600"),
            ("encode", "on"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let caddy = CADDY.render(&c);
        assert!(caddy.contains("\trequest_body {\n\t\tmax_size 64MiB\n\t}\n\tencode zstd gzip\n}"));
        assert!(caddy.contains("\tphp_fastcgi {args[0]} {\n\t\tread_timeout 300s\n\t}\n"));
        assert!(caddy.contains("\tservers {\n\t\ttimeouts {\n\t\t\tidle 600s\n"));
        assert!(
            caddy.find("idle 600s").unwrap() < caddy.find("(devluma_settings)").unwrap(),
            "server timeouts are global options"
        );
        assert!(caddy.find("(devluma_php)").unwrap() < caddy.find("import \"").unwrap());
        c.settings = [("php_read_timeout".to_owned(), "1}\nevil".to_owned())].into();
        assert!(!CADDY.render(&c).contains("evil"));
    }

    #[test]
    fn mariadb_messages_drop_timestamps_and_paths() {
        let out = "2026-10-22  0:26:04 0 [ERROR] C:\\x\\bin\\mariadbd.exe: unknown variable 'foo_bar=1'\n\
                   2026-10-22  0:26:04 0 [ERROR] Aborting\n\
                   2026-10-22  0:26:05 0 [Warning] Buffered warning: option 'max_connections': unsigned value 99999999 adjusted to 100000\n\
                   \x20                     Versioning ALTER TABLE mode. ERROR: Fail ALTER with\n";
        let (errors, warnings) = mariadb_messages(out);
        assert_eq!(errors, vec!["unknown variable 'foo_bar=1'"]);
        assert_eq!(
            warnings,
            vec!["option 'max_connections': unsigned value 99999999 adjusted to 100000"]
        );
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
    fn caddy_config_is_local_only_and_never_issues_or_trusts_certificates() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        c.install_dir = PathBuf::from(r"C:\Devluma\packages\caddy\2.11.9-x64");
        c.admin_port = Some(2020);
        CADDY.prepare(&c).unwrap();
        let conf = std::fs::read_to_string(CADDY.config_path(&c)).unwrap();
        for line in [
            "\tadmin 127.0.0.1:2020\n",
            "\tauto_https off\n",
            "\tskip_install_trust\n",
            "\tdefault_bind 127.0.0.1\n",
            "\tpersist_config off\n",
            "http://:8080 {\n",
        ] {
            assert!(conf.contains(line), "missing {line:?} in\n{conf}");
        }
        assert!(conf.starts_with("# Managed by Devluma"));
        assert!(
            conf.find("\tadmin").unwrap() < conf.find("http://:").unwrap(),
            "global options must be the first block"
        );
        assert!(!conf.contains('\\'), "Caddyfile paths use forward slashes");
        assert!(!conf.contains("    "), "Caddyfile indentation is tabs");
        assert!(conf.ends_with("/sites/*.conf\"\n"));

        let launch = CADDY.launch(&c);
        let args: Vec<String> = launch
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "run");
        assert!(args.contains(&"caddyfile".to_owned()));
        let data = launch
            .env
            .iter()
            .find(|(k, _)| k == "XDG_DATA_HOME")
            .unwrap();
        assert!(
            Path::new(&data.1).starts_with(dir.path()),
            "data stays in the service folder"
        );
        assert!(launch.env.iter().any(|(k, _)| k == "XDG_CONFIG_HOME"));

        std::fs::write(dir.path().join("conf").join("custom.caddy"), "# mine\n").unwrap();
        CADDY.prepare(&c).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("conf").join("custom.caddy")).unwrap(),
            "# mine\n",
            "user-owned custom.caddy is never overwritten"
        );
        assert_eq!(CADDY.admin_port().unwrap().default, 2019);
    }

    #[test]
    fn frankenphp_uses_php_server_and_its_own_php_ini() {
        let dir = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        std::fs::create_dir(install.path().join("ext")).unwrap();
        for ext in ["mbstring", "openssl"] {
            std::fs::write(
                install.path().join("ext").join(format!("php_{ext}.dll")),
                b"",
            )
            .unwrap();
        }
        std::fs::write(
            install.path().join("php.ini-development"),
            "memory_limit = 128M\r\n;extension=mbstring\r\n;extension=openssl\r\n;extension=imap\r\n",
        )
        .unwrap();
        let mut c = ctx(dir.path());
        c.install_dir = install.path().to_path_buf();
        c.admin_port = Some(2021);
        c.settings = [("php_read_timeout".to_owned(), "300".to_owned())].into();
        FRANKENPHP.prepare(&c).unwrap();

        let conf = std::fs::read_to_string(FRANKENPHP.config_path(&c)).unwrap();
        assert!(
            conf.contains("(devluma_php) {\n\tphp_server\n}\n"),
            "{conf}"
        );
        assert!(!conf.contains("php_fastcgi") && !conf.contains("read_timeout"));
        assert!(conf.contains("\tauto_https off\n") && conf.contains("\tadmin 127.0.0.1:2021\n"));

        let ini = std::fs::read_to_string(Caddy::php_ini(&c)).unwrap();
        assert!(ini.contains(&format!(
            "extension_dir = \"{}\"",
            install.path().join("ext").display()
        )));
        assert!(
            ini.contains("\nextension=mbstring\r\n") && ini.contains("\nextension=openssl\r\n")
        );
        assert!(
            ini.contains(";extension=imap"),
            "not shipped, so not enabled"
        );
        assert!(
            !install.path().join("php.ini").exists(),
            "nothing is written into the user's FrankenPHP folder"
        );

        std::fs::write(Caddy::php_ini(&c), "memory_limit = 512M\n").unwrap();
        FRANKENPHP.prepare(&c).unwrap();
        assert_eq!(
            std::fs::read_to_string(Caddy::php_ini(&c)).unwrap(),
            "memory_limit = 512M\n",
            "an existing php.ini is the user's"
        );

        let launch = FRANKENPHP.launch(&c);
        assert!(launch.exe.ends_with("frankenphp.exe"));
        let phprc = launch.env.iter().find(|(k, _)| k == "PHPRC").unwrap();
        assert_eq!(Path::new(&phprc.1), dir.path().join("php"));
        assert!(!CADDY.launch(&c).env.iter().any(|(k, _)| k == "PHPRC"));
        let keys: Vec<_> = FRANKENPHP.config_files(&c).iter().map(|f| f.key).collect();
        assert_eq!(keys, ["custom", "php", "main"]);
    }

    #[test]
    fn caddy_errors_drop_json_log_lines() {
        let out = winproc::Output {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "{\"level\":\"info\",\"msg\":\"using config from file\"}\nError: adapting config using caddyfile: unrecognized directive: nonsense\n".into(),
        };
        assert_eq!(
            Caddy::error_text(&out),
            "Error: adapting config using caddyfile: unrecognized directive: nonsense"
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
