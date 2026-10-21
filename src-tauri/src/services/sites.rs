//! Local sites (app_flow.md §8): a project folder served by an Nginx service at a local
//! domain, optionally with PHP through a FastCGI backend (ADR 0009).
//!
//! Apply: preview (exact config + hosts diff) → write the site config with a backup →
//! `nginx -t` → reload if running → hosts entry (may prompt for admin) → HTTP health check.
//! If validation or reload fails, the previous config is restored, so an invalid config never
//! replaces a working one. Removal takes the hosts entry out first, so a declined admin prompt
//! leaves the site exactly as it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::certificates::CertificateService;
use super::hosts::{HostsPlan, HostsService};
use super::journal::{Journal, ProgressSink};
use super::supervisor::Supervisor;
use super::supervisor::behavior::{Failure, failure};
use crate::domain::domain_name::{self, DomainCheck};
use crate::domain::hosts::ManagedEntry;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::certificates as certs;
use crate::persistence::installations;
use crate::persistence::operations::{EventLevel, NewOperation};
use crate::persistence::projects;
use crate::persistence::services::ServiceInstance;
use crate::persistence::sites::{self, Site};
use crate::persistence::{Database, DbError, new_id};
use crate::security::paths as pathpolicy;
use crate::state::DataPaths;

const LOOPBACK: &str = "127.0.0.1";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteRequest {
    /// Set when editing an existing site.
    #[serde(default)]
    pub site_id: Option<String>,
    pub project_id: String,
    pub domain: String,
    pub web_server_service_id: String,
    /// Serve PHP with the project's effective PHP version.
    pub use_php: bool,
    /// Absolute folder inside the project; defaults to the detected document root.
    #[serde(default)]
    pub document_root: Option<String>,
    pub manage_hosts: bool,
    /// SHA-256 of the hosts file shown in the preview; apply refuses if it changed.
    #[serde(default)]
    pub expected_hosts_sha256: Option<String>,
    /// Also serve over HTTPS with a certificate from Devluma's local CA.
    #[serde(default)]
    pub https: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpsPlan {
    pub port: u16,
    pub url: String,
    /// Existing certificate that will be used; `None` means one is issued on apply.
    pub certificate_id: Option<String>,
    pub ca_exists: bool,
    pub ca_trusted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpChoice {
    pub installation_id: String,
    pub version: String,
    /// Existing backend port, or the one that would be assigned.
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SitePlan {
    pub site_id: String,
    pub domain: DomainCheck,
    pub url: String,
    pub document_root: String,
    pub config_path: String,
    pub config_text: String,
    pub config_diff: String,
    pub php: Option<PhpChoice>,
    pub https: Option<HttpsPlan>,
    pub hosts: Option<HostsPlan>,
    pub web_server_running: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteView {
    #[serde(flatten)]
    pub site: Site,
    pub project_name: String,
    pub web_server_name: String,
    pub web_server_running: bool,
    pub url: String,
    pub https_url: Option<String>,
    pub php_version: Option<String>,
    /// The project's effective PHP differs from the one this site was applied with.
    pub php_outdated: bool,
    pub config_present: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteCheck {
    pub url: String,
    pub status: Option<u16>,
    pub message: String,
}

pub struct SiteService {
    db: Arc<Database>,
    paths: DataPaths,
    supervisor: Arc<Supervisor>,
    hosts: Arc<HostsService>,
    certificates: Arc<CertificateService>,
    progress: Arc<dyn ProgressSink>,
}

fn ng(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn url_for(domain: &str, port: i64) -> String {
    if port == 80 {
        format!("http://{domain}/")
    } else {
        format!("http://{domain}:{port}/")
    }
}

fn version_of(package_version_id: &str) -> String {
    package_version_id
        .split('@')
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}

/// Response header every site's server block adds, so a health check can prove the answer
/// came from *this* site's configuration (not the default server or a not-yet-reloaded worker).
pub const SITE_HEADER: &str = "X-Devluma-Site";

/// Minimal blocking HTTP/1.1 GET to 127.0.0.1:`port` with `Host: domain`. Returns the status
/// code and the `X-Devluma-Site` header, if any. No hosts entry or async runtime needed.
fn http_probe(domain: &str, port: u16) -> std::io::Result<(u16, Option<String>)> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let host = if port == 80 {
        domain.to_owned()
    } else {
        format!("{domain}:{port}")
    };
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Devluma\r\nConnection: close\r\n\r\n"
    )?;
    // Read just the response head (bounded).
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while head.len() < 16 * 1024 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .filter(|_| status_line.starts_with("HTTP/"))
        .ok_or_else(|| std::io::Error::other("not an HTTP response"))?;
    let site = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(SITE_HEADER))
        .map(|(_, v)| v.trim().trim_matches('"').to_owned());
    Ok((status, site))
}

/// Which web server a site's config is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServerKind {
    Nginx,
    Apache,
}

impl WebServerKind {
    pub fn of(svc: &ServiceInstance) -> Option<Self> {
        match svc.lifecycle_adapter.as_str() {
            "nginx" => Some(Self::Nginx),
            "apache" => Some(Self::Apache),
            _ => None,
        }
    }
}

/// A site's server block (nginx) or virtual hosts (Apache). Every interpolated value is
/// validated beforehand (domain by `domain_name::check`, paths by `check_document_root`).
pub struct SiteConfig<'a> {
    pub server: WebServerKind,
    pub site_id: &'a str,
    pub domain: &'a str,
    pub http_port: u16,
    pub document_root: &'a Path,
    /// The web server's install folder (nginx: `conf/fastcgi_params`).
    pub web_install: &'a Path,
    /// The web server service's folder (Apache: absolute log paths).
    pub service_dir: &'a Path,
    pub php_port: Option<u16>,
    pub https: Option<HttpsConfig>,
}

#[derive(Debug, Clone)]
pub struct HttpsConfig {
    pub port: u16,
    pub certificate: PathBuf,
    pub key: PathBuf,
}

pub fn render_config(c: &SiteConfig<'_>) -> String {
    match c.server {
        WebServerKind::Nginx => render_nginx(c),
        WebServerKind::Apache => render_apache(c),
    }
}

fn render_apache(c: &SiteConfig<'_>) -> String {
    let (site_id, domain) = (c.site_id, c.domain);
    let root = ng(c.document_root);
    let logs = ng(&c.service_dir.join("logs"));
    let mut body = format!(
        "  ServerName {domain}\n\
         \x20 DocumentRoot \"{root}\"\n\
         \x20 ErrorLog \"{logs}/site-{site_id}.error.log\"\n\
         \x20 CustomLog \"{logs}/site-{site_id}.access.log\" common\n\
         \x20 Header always set {SITE_HEADER} \"{site_id}\"\n\
         \x20 <Directory \"{root}\">\n\
         \x20   Options FollowSymLinks\n\
         \x20   AllowOverride All\n\
         \x20   Require all granted\n\
         \x20 </Directory>\n",
    );
    match c.php_port {
        // Verified against Apache Lounge 2.4.69 + php-cgi on Windows:
        // - trailing slash: Apache appends the script path, and "…:9001C:/…" isn't an address;
        // - GENERIC: php-cgi isn't PHP-FPM;
        // - Apache then sends SCRIPT_FILENAME as "/C:/…", which PHP can't open ("No input
        //   file specified"), so the leading slash is removed.
        Some(p) => body.push_str(&format!(
            "  DirectoryIndex index.php index.html index.htm\n\
             \x20 <FilesMatch \"\\.php$\">\n\
             \x20   SetHandler \"proxy:fcgi://{LOOPBACK}:{p}/\"\n\
             \x20 </FilesMatch>\n\
             \x20 ProxyFCGIBackendType GENERIC\n\
             \x20 ProxyFCGISetEnvIf \"reqenv('SCRIPT_FILENAME') =~ m#^/([A-Za-z]:/.*)$#\" SCRIPT_FILENAME \"$1\"\n",
        )),
        None => body.push_str("  DirectoryIndex index.html index.htm\n"),
    }
    let mut out = format!(
        "# Managed by Devluma for {domain} (site {site_id}). Regenerated whenever the site is applied.\n\
         <VirtualHost {LOOPBACK}:{port}>\n{body}</VirtualHost>\n",
        port = c.http_port,
    );
    if let Some(h) = &c.https {
        // Every HTTPS site on this port declares the Listen; the Define makes only the first
        // one count (Apache refuses duplicate Listen directives).
        out.push_str(&format!(
            "<IfDefine !devluma_https_{port}>\n\
             \x20 Define devluma_https_{port}\n\
             \x20 Listen {LOOPBACK}:{port} https\n\
             </IfDefine>\n\
             <VirtualHost {LOOPBACK}:{port}>\n\
             \x20 SSLEngine on\n\
             \x20 SSLCertificateFile \"{cert}\"\n\
             \x20 SSLCertificateKeyFile \"{key}\"\n{body}</VirtualHost>\n",
            port = h.port,
            cert = ng(&h.certificate),
            key = ng(&h.key),
        ));
    }
    out
}

fn render_nginx(c: &SiteConfig<'_>) -> String {
    let (site_id, domain) = (c.site_id, c.domain);
    let mut body = format!(
        "  server_name {domain};\n\
         \x20 root \"{root}\";\n\
         \x20 access_log logs/site-{site_id}.access.log;\n\
         \x20 error_log logs/site-{site_id}.error.log;\n\
         \x20 add_header {SITE_HEADER} \"{site_id}\" always;\n",
        root = ng(c.document_root),
    );
    match c.php_port {
        Some(p) => body.push_str(&format!(
            "  index index.php index.html index.htm;\n\
             \x20 location / {{\n\
             \x20   try_files $uri $uri/ /index.php?$query_string;\n\
             \x20 }}\n\
             \x20 location ~ \\.php$ {{\n\
             \x20   try_files $uri =404;\n\
             \x20   fastcgi_pass {LOOPBACK}:{p};\n\
             \x20   fastcgi_index index.php;\n\
             \x20   include \"{params}\";\n\
             \x20   fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n\
             \x20 }}\n",
            params = ng(&c.web_install.join("conf").join("fastcgi_params")),
        )),
        None => body.push_str("  index index.html index.htm;\n"),
    }
    let mut out = format!(
        "# Managed by Devluma for {domain} (site {site_id}). Regenerated whenever the site is applied.\n\
         server {{\n\
         \x20 listen {LOOPBACK}:{port};\n{body}}}\n",
        port = c.http_port,
    );
    if let Some(h) = &c.https {
        out.push_str(&format!(
            "server {{\n\
             \x20 listen {LOOPBACK}:{port} ssl;\n\
             \x20 ssl_certificate \"{cert}\";\n\
             \x20 ssl_certificate_key \"{key}\";\n{body}}}\n",
            port = h.port,
            cert = ng(&h.certificate),
            key = ng(&h.key),
        ));
    }
    out
}

/// Document roots go into nginx config inside quotes: refuse anything that could escape them
/// or be interpreted as a variable.
fn check_document_root(project_root: &Path, input: &Path) -> Result<PathBuf, Failure> {
    let s = input.to_string_lossy();
    if s.chars()
        .any(|c| c.is_control() || matches!(c, '"' | '$' | '{' | '}' | ';'))
    {
        return Err(failure(
            ErrorCode::ValidationError,
            "The document root contains characters that can't be used in a web server config (\" $ { } ;).",
        ));
    }
    if !input.is_dir() {
        return Err(failure(
            ErrorCode::ValidationError,
            format!("{} isn't an existing folder.", input.display()),
        ));
    }
    pathpolicy::ensure_within(project_root, input).map_err(|_| {
        failure(
            ErrorCode::ValidationError,
            "The document root must be inside the project folder.",
        )
    })?;
    Ok(input.to_path_buf())
}

impl SiteService {
    pub fn new(
        db: Arc<Database>,
        paths: DataPaths,
        supervisor: Arc<Supervisor>,
        hosts: Arc<HostsService>,
        certificates: Arc<CertificateService>,
        progress: Arc<dyn ProgressSink>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            paths,
            supervisor,
            hosts,
            certificates,
            progress,
        })
    }

    /// HTTPS port for a site on `web`: shared with the server's other HTTPS sites, else the
    /// one chosen in Settings, else 443 (when HTTP is on 80) or 8443. A new port must be free.
    fn https_port(&self, web: &ServiceInstance) -> Result<u16, Failure> {
        let (all, preferred) = self
            .db
            .read(|c| Ok((sites::list(c)?, super::preferences::load(c)?.https_port)))
            .map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        if let Some(p) = all
            .iter()
            .filter(|s| s.web_server_service_id == web.id && s.ssl_enabled)
            .find_map(|s| s.https_port)
            .and_then(|p| u16::try_from(p).ok())
        {
            return Ok(p);
        }
        let port = preferred.unwrap_or(if web.port == Some(80) { 443 } else { 8443 });
        if web.port == Some(i64::from(port)) {
            return Err(failure(
                ErrorCode::ValidationError,
                format!(
                    "The HTTPS port in Settings ({port}) is the web server's HTTP port. Choose another HTTPS port."
                ),
            ));
        }
        if let Some(who) = crate::platform::windows::ports::used_by(port) {
            return Err(failure(
                ErrorCode::PortInUse,
                format!(
                    "HTTPS port {port} is already used by {who}. Stop it or use a web server on another port."
                ),
            ));
        }
        Ok(port)
    }

    fn web_server(&self, id: &str) -> Result<ServiceInstance, Failure> {
        let svc = self.supervisor.service(id)?;
        if WebServerKind::of(&svc).is_none() {
            return Err(failure(
                ErrorCode::ValidationError,
                "Sites need a web server service (Nginx or Apache).",
            ));
        }
        Ok(svc)
    }

    fn config_path(&self, web_id: &str, site_id: &str) -> PathBuf {
        self.supervisor
            .service_dir(web_id)
            .join("sites")
            .join(format!("{site_id}.conf"))
    }

    /// Everything the apply would do, computed without changing anything.
    pub fn preview(&self, req: &SiteRequest) -> Result<SitePlan, Failure> {
        let db_fail = |e: DbError| {
            let i = IpcError::from(e);
            failure(i.code, i.message)
        };
        let project = self
            .db
            .read(|c| projects::get(c, &req.project_id))
            .map_err(db_fail)?;
        let domain =
            domain_name::check(&req.domain).map_err(|m| failure(ErrorCode::ValidationError, m))?;
        let site_id = req.site_id.clone().unwrap_or_else(new_id);
        if let Some(other) = self
            .db
            .read(|c| sites::find_by_domain(c, &domain.normalized))
            .map_err(db_fail)?
            && other.id != site_id
        {
            return Err(failure(
                ErrorCode::ValidationError,
                format!("{} is already used by another site.", domain.normalized),
            ));
        }

        let web = self.web_server(&req.web_server_service_id)?;
        let port = web
            .port
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| failure(ErrorCode::ValidationError, "The web server has no port."))?;
        let server = WebServerKind::of(&web).unwrap_or(WebServerKind::Nginx);
        let service_dir = self.supervisor.service_dir(&web.id);
        let web_install = PathBuf::from(
            self.db
                .read(|c| installations::get(c, &web.package_installation_id))
                .map_err(db_fail)?
                .install_path,
        );

        let project_root = PathBuf::from(&project.root_path);
        let docroot_input = req
            .document_root
            .as_ref()
            .filter(|d| !d.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(super::projects::detect(&project_root).suggested_document_root)
            });
        let document_root = check_document_root(&project_root, &docroot_input)?;

        let php = if req.use_php {
            let sel = self
                .db
                .read(|c| super::runtime::effective(c, Some(&project.id), "php"))
                .map_err(db_fail)?
                .ok_or_else(|| {
                    failure(
                        ErrorCode::ValidationError,
                        format!(
                            "{} has no PHP version: pin one on the Projects page or make a version the default.",
                            project.name
                        ),
                    )
                })?;
            let port = self.supervisor.php_backend_port(&sel.installation_id)?;
            Some(PhpChoice {
                installation_id: sel.installation_id,
                version: sel.version,
                port,
            })
        } else {
            None
        };

        let https = if req.https {
            CertificateService::check_domains(std::slice::from_ref(&domain.normalized))?;
            let https_port = self.https_port(&web)?;
            let existing = self.certificates.find_for_domain(&domain.normalized)?;
            let ca = self.certificates.ca()?;
            Some((
                HttpsPlan {
                    port: https_port,
                    url: if https_port == 443 {
                        format!("https://{}/", domain.normalized)
                    } else {
                        format!("https://{}:{https_port}/", domain.normalized)
                    },
                    certificate_id: existing.as_ref().map(|c| c.id.clone()),
                    ca_trusted: ca
                        .as_ref()
                        .is_some_and(|c| self.certificates.ca_trusted_now(c)),
                    ca_exists: ca.is_some(),
                },
                existing,
            ))
        } else {
            None
        };
        // Until a certificate exists, the preview shows where it will be written.
        let https_config = https.as_ref().map(|(h, existing)| {
            let (certificate, key) = match existing {
                Some(c) => (
                    PathBuf::from(&c.certificate_path),
                    PathBuf::from(&c.private_key_path),
                ),
                None => {
                    let dir = self
                        .paths
                        .root
                        .join("certificates")
                        .join("(issued on apply)");
                    (dir.join("cert.pem"), dir.join("key.pem"))
                }
            };
            HttpsConfig {
                port: h.port,
                certificate,
                key,
            }
        });

        let config_path = self.config_path(&web.id, &site_id);
        let config_text = render_config(&SiteConfig {
            site_id: &site_id,
            domain: &domain.normalized,
            http_port: port,
            document_root: &document_root,
            server,
            web_install: &web_install,
            service_dir: &service_dir,
            php_port: php.as_ref().map(|p| p.port),
            https: https_config,
        });
        let current = std::fs::read_to_string(&config_path).unwrap_or_default();
        let config_diff =
            crate::domain::hosts::diff(&current, &config_text, &config_path.display().to_string());

        let hosts = if req.manage_hosts {
            Some(self.hosts.plan_change(
                &[ManagedEntry {
                    ip: LOOPBACK.into(),
                    domain: domain.normalized.clone(),
                }],
                &[],
            )?)
        } else {
            None
        };

        let mut warnings = domain.warnings.clone();
        let running = matches!(
            web.observed_state,
            crate::domain::service::ServiceState::Running
                | crate::domain::service::ServiceState::Degraded
        );
        if !running {
            warnings.push(format!(
                "{} isn't running; start it to serve the site.",
                web.instance_name
            ));
        }
        if !req.manage_hosts {
            warnings.push(format!(
                "Without a hosts entry, {} only resolves if you add it yourself.",
                domain.normalized
            ));
        }
        Ok(SitePlan {
            url: url_for(&domain.normalized, i64::from(port)),
            site_id,
            domain,
            document_root: document_root.display().to_string(),
            config_path: config_path.display().to_string(),
            config_text,
            config_diff,
            php,
            https: https.map(|(h, _)| h),
            hosts,
            web_server_running: running,
            warnings,
        })
    }

    /// Starts the journaled apply; returns the operation id.
    pub fn apply(
        self: &Arc<Self>,
        req: SiteRequest,
    ) -> Result<(String, tauri::async_runtime::JoinHandle<()>), IpcError> {
        let plan = self
            .preview(&req)
            .map_err(|f| IpcError::new(f.code, f.message, true))?;
        if let (Some(expected), Some(h)) = (&req.expected_hosts_sha256, &plan.hosts)
            && expected != &h.sha256_before
        {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "The hosts file changed since the preview. Review the change again.",
                true,
            ));
        }
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let op = journal.start(&NewOperation {
            operation_type: "apply_site",
            target_type: Some("site"),
            target_id: Some(&plan.site_id),
        })?;
        let (me, op_id) = (Arc::clone(self), op.id.clone());
        let handle = tauri::async_runtime::spawn_blocking(move || {
            let journal = Journal::new(&me.db, me.progress.as_ref());
            let _ = journal.running(&op_id);
            let _ = match me.do_apply(&op_id, &req, plan) {
                Ok(()) => journal.succeed(&op_id).map(|_| ()),
                Err(f) => journal.fail(&op_id, f.code, &f.message).map(|_| ()),
            };
        });
        Ok((op.id, handle))
    }

    fn do_apply(
        self: &Arc<Self>,
        op: &str,
        req: &SiteRequest,
        mut plan: SitePlan,
    ) -> Result<(), Failure> {
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let db_fail = |e: DbError| {
            let i = IpcError::from(e);
            failure(i.code, i.message)
        };

        // 1. PHP backend and certificate (created on demand), then the final config.
        let web = self.web_server(&req.web_server_service_id)?;
        let server = WebServerKind::of(&web).unwrap_or(WebServerKind::Nginx);
        let service_dir = self.supervisor.service_dir(&web.id);
        let web_install = PathBuf::from(
            self.db
                .read(|c| installations::get(c, &web.package_installation_id))
                .map_err(db_fail)?
                .install_path,
        );
        if let Some(php) = &mut plan.php {
            let _ = journal.progress(op, "Starting the PHP backend", None, None);
            let backend = self
                .supervisor
                .ensure_php_backend(&php.installation_id, op)?;
            php.port = backend
                .port
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(php.port);
        }
        let certificate = match &plan.https {
            Some(_) => {
                let _ = journal.progress(op, "Preparing the certificate", None, None);
                Some(self.certificates.for_domain(&plan.domain.normalized)?)
            }
            None => None,
        };
        plan.config_text = render_config(&SiteConfig {
            site_id: &plan.site_id,
            domain: &plan.domain.normalized,
            http_port: web.port.and_then(|p| u16::try_from(p).ok()).unwrap_or(80),
            document_root: Path::new(&plan.document_root),
            server,
            web_install: &web_install,
            service_dir: &service_dir,
            php_port: plan.php.as_ref().map(|p| p.port),
            https: plan
                .https
                .as_ref()
                .zip(certificate.as_ref())
                .map(|(h, c)| HttpsConfig {
                    port: h.port,
                    certificate: PathBuf::from(&c.certificate_path),
                    key: PathBuf::from(&c.private_key_path),
                }),
        });

        // 2. Config: backup → write → nginx -t → reload; any failure restores the previous file.
        let _ = journal.progress(
            op,
            "Writing and validating the site configuration",
            None,
            None,
        );
        let config_path = PathBuf::from(&plan.config_path);
        let previous = std::fs::read_to_string(&config_path).ok();
        let restore = |prev: &Option<String>| match prev {
            Some(text) => {
                let _ = std::fs::write(&config_path, text);
            }
            None => {
                let _ = std::fs::remove_file(&config_path);
            }
        };
        if let Some(prev) = &previous
            && prev != &plan.config_text
        {
            let dir = self.paths.backups.join("sites");
            let _ = std::fs::create_dir_all(&dir);
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
            std::fs::write(dir.join(format!("{}-{stamp}.conf", plan.site_id)), prev).map_err(
                |e| {
                    failure(
                        ErrorCode::Internal,
                        format!("Could not back up the previous site config: {e}"),
                    )
                },
            )?;
        }
        if let Some(dir) = config_path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        }
        // Temp name doesn't match `sites/*.conf`, so nginx never reads a half-written file.
        let tmp = config_path.with_extension("conf.devluma-tmp");
        std::fs::write(&tmp, &plan.config_text)
            .and_then(|_| std::fs::rename(&tmp, &config_path))
            .map_err(|e| {
                failure(
                    ErrorCode::Internal,
                    format!("Could not write the site config: {e}"),
                )
            })?;
        if let Err(f) = self.supervisor.validate_config(&req.web_server_service_id) {
            restore(&previous);
            return Err(failure(
                f.code,
                format!("{} The previous configuration was kept.", f.message),
            ));
        }
        match self
            .supervisor
            .reload_if_running(&req.web_server_service_id)
        {
            Ok(true) => {
                let _ = journal.event(
                    op,
                    EventLevel::Info,
                    "Web server reloaded",
                    &serde_json::json!({}),
                );
            }
            Ok(false) => {}
            Err(f) => {
                restore(&previous);
                let _ = self
                    .supervisor
                    .reload_if_running(&req.web_server_service_id);
                return Err(failure(
                    f.code,
                    format!("{} The previous configuration was restored.", f.message),
                ));
            }
        }

        // 3. Save the site (config is live from here on).
        let mut site = Site {
            id: plan.site_id.clone(),
            project_id: req.project_id.clone(),
            domain: plan.domain.normalized.clone(),
            document_root: plan.document_root.clone(),
            web_server_service_id: web.id.clone(),
            php_installation_id: plan.php.as_ref().map(|p| p.installation_id.clone()),
            http_port: web.port.unwrap_or(80),
            https_port: plan.https.as_ref().map(|h| i64::from(h.port)),
            ssl_enabled: plan.https.is_some(),
            config_path: plan.config_path.clone(),
            config_hash: Some(crate::platform::windows::hosts_file::sha256_hex(
                plan.config_text.as_bytes(),
            )),
            status: "active".into(),
            hosts_managed: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let was_managed = self
            .db
            .read(|c| sites::get(c, &site.id))
            .map(|s| s.hosts_managed)
            .unwrap_or(false);
        site.hosts_managed = was_managed;
        self.db
            .write(|c| {
                let saved = sites::upsert(c, &site)?;
                match &certificate {
                    Some(cert) => certs::bind(c, &saved.id, &cert.id)?,
                    None => certs::unbind_site(c, &saved.id)?,
                }
                Ok(saved)
            })
            .map_err(db_fail)?;

        // 4. Hosts entry (may show a Windows administrator prompt).
        if let Some(plan_hosts) = &plan.hosts {
            let _ = journal.progress(
                op,
                if plan_hosts.needs_elevation {
                    "Waiting for administrator approval (hosts file)"
                } else {
                    "Updating the hosts file"
                },
                None,
                None,
            );
            if let Err(f) = self
                .hosts
                .apply(plan_hosts, &format!("Site {}", site.domain), op)
            {
                return Err(failure(
                    f.code,
                    format!(
                        "The site configuration was applied, but the hosts entry wasn't added: {} You can retry from the site.",
                        f.message
                    ),
                ));
            }
            site.hosts_managed = true;
            self.db
                .write(|c| sites::upsert(c, &site))
                .map_err(db_fail)?;
        }

        // 5. Health check (only meaningful if the web server is running).
        if self
            .supervisor
            .service(&web.id)
            .map(|s| s.observed_state.is_live())
            .unwrap_or(false)
        {
            let check = self.check_site(&site.id);
            let _ = journal.event(
                op,
                EventLevel::Info,
                &check.message,
                &serde_json::json!({ "status": check.status }),
            );
        }
        Ok(())
    }

    /// HTTP GET against the site through loopback with the site's Host header.
    pub fn check_site(&self, site_id: &str) -> SiteCheck {
        let Ok(site) = self.db.read(|c| sites::get(c, site_id)) else {
            return SiteCheck {
                url: String::new(),
                status: None,
                message: "Site not found.".into(),
            };
        };
        let url = url_for(&site.domain, site.http_port);
        let port = u16::try_from(site.http_port).unwrap_or(80);
        // A reload is asynchronous: give new workers a moment to pick up the config.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match http_probe(&site.domain, port) {
                Ok((status, Some(id))) if id == site.id => {
                    return SiteCheck {
                        message: format!("{url} answered with HTTP {status}."),
                        url,
                        status: Some(status),
                    };
                }
                Ok((status, _)) if std::time::Instant::now() >= deadline => {
                    return SiteCheck {
                        message: format!(
                            "{url} answered with HTTP {status}, but not from this site's configuration. Check that the web server is running and has been reloaded."
                        ),
                        url,
                        status: None,
                    };
                }
                Err(e) if std::time::Instant::now() >= deadline => {
                    return SiteCheck {
                        message: format!("{url} didn't answer: {e}"),
                        url,
                        status: None,
                    };
                }
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    }

    /// Removes a site: hosts entry first (so a declined prompt changes nothing), then config.
    pub fn remove(
        self: &Arc<Self>,
        site_id: &str,
    ) -> Result<(String, tauri::async_runtime::JoinHandle<()>), IpcError> {
        let site = self.db.read(|c| sites::get(c, site_id))?;
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let op = journal.start(&NewOperation {
            operation_type: "remove_site",
            target_type: Some("site"),
            target_id: Some(site_id),
        })?;
        let (me, op_id) = (Arc::clone(self), op.id.clone());
        let handle = tauri::async_runtime::spawn_blocking(move || {
            let journal = Journal::new(&me.db, me.progress.as_ref());
            let _ = journal.running(&op_id);
            let result = (|| -> Result<(), Failure> {
                if site.hosts_managed {
                    let plan = me
                        .hosts
                        .plan_change(&[], std::slice::from_ref(&site.domain))?;
                    let _ = journal.progress(&op_id, "Removing the hosts entry", None, None);
                    me.hosts
                        .apply(&plan, &format!("Remove site {}", site.domain), &op_id)?;
                }
                let path = PathBuf::from(&site.config_path);
                let previous = std::fs::read_to_string(&path).ok();
                if let Some(prev) = &previous {
                    let dir = me.paths.backups.join("sites");
                    let _ = std::fs::create_dir_all(&dir);
                    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
                    let _ = std::fs::write(dir.join(format!("{}-{stamp}.conf", site.id)), prev);
                    let _ = std::fs::remove_file(&path);
                }
                let _ = me.supervisor.reload_if_running(&site.web_server_service_id);
                me.db
                    .write(|c| sites::delete(c, &site.id))
                    .map_err(|e| failure(ErrorCode::Internal, e.to_string()))
            })();
            let _ = match result {
                Ok(()) => journal.succeed(&op_id).map(|_| ()),
                Err(f) => journal.fail(&op_id, f.code, &f.message).map(|_| ()),
            };
        });
        Ok((op.id, handle))
    }

    pub fn list(&self) -> Result<Vec<SiteView>, IpcError> {
        let (all, projects, insts) = self
            .db
            .read(|c| Ok((sites::list(c)?, projects::list(c)?, installations::list(c)?)))?;
        Ok(all
            .into_iter()
            .map(|site| {
                let project_name = projects.iter().find(|p| p.id == site.project_id).map(|p| p.name.clone()).unwrap_or_default();
                let web = self.supervisor.service(&site.web_server_service_id).ok();
                let php_version = site
                    .php_installation_id
                    .as_ref()
                    .and_then(|id| insts.iter().find(|i| &i.id == id))
                    .map(|i| version_of(&i.package_version_id));
                let effective = site.php_installation_id.as_ref().and_then(|_| {
                    self.db
                        .read(|c| super::runtime::effective(c, Some(&site.project_id), "php"))
                        .ok()
                        .flatten()
                });
                SiteView {
                    url: url_for(&site.domain, site.http_port),
                    https_url: site.https_port.filter(|_| site.ssl_enabled).map(|p| {
                        if p == 443 {
                            format!("https://{}/", site.domain)
                        } else {
                            format!("https://{}:{p}/", site.domain)
                        }
                    }),
                    php_outdated: matches!((&site.php_installation_id, &effective), (Some(a), Some(b)) if a != &b.installation_id),
                    config_present: Path::new(&site.config_path).is_file(),
                    web_server_name: web.as_ref().map(|w| w.instance_name.clone()).unwrap_or_default(),
                    web_server_running: web.as_ref().is_some_and(|w| w.observed_state.is_live()),
                    project_name,
                    php_version,
                    site,
                }
            })
            .collect())
    }

    /// Opens a site's URL in the default browser. Only URLs of existing sites can be opened.
    pub fn open(&self, site_id: &str) -> Result<String, IpcError> {
        let site = self.db.read(|c| sites::get(c, site_id))?;
        let url = url_for(&site.domain, site.http_port);
        crate::platform::windows::process::open_url(&url).map_err(|e| {
            IpcError::new(
                ErrorCode::Internal,
                format!("Could not open the browser: {e}"),
                true,
            )
        })?;
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_static_php_and_https_configs() {
        let base = SiteConfig {
            server: WebServerKind::Nginx,
            service_dir: Path::new(r"C:\Devluma\services\w1"),
            site_id: "s1",
            domain: "shop.test",
            http_port: 8080,
            document_root: Path::new(r"D:\code\shop\public"),
            web_install: Path::new(r"C:\Devluma\packages\nginx\1.30.5-x64"),
            php_port: None,
            https: None,
        };
        let s = render_config(&base);
        assert!(s.contains("listen 127.0.0.1:8080;") && s.contains("server_name shop.test;"));
        assert!(s.contains("root \"D:/code/shop/public\";"));
        assert!(!s.contains("fastcgi_pass") && !s.contains("ssl"));

        let p = render_config(&SiteConfig {
            php_port: Some(9001),
            ..base
        });
        assert!(p.contains("fastcgi_pass 127.0.0.1:9001;"));
        assert!(
            p.contains("include \"C:/Devluma/packages/nginx/1.30.5-x64/conf/fastcgi_params\";")
        );
        assert!(p.contains("location ~ \\.php$"));

        let h = render_config(&SiteConfig {
            php_port: Some(9001),
            https: Some(HttpsConfig {
                port: 8443,
                certificate: PathBuf::from(r"C:\c\cert.pem"),
                key: PathBuf::from(r"C:\c\key.pem"),
            }),
            ..base
        });
        assert_eq!(h.matches("server {").count(), 2, "HTTP and HTTPS blocks");
        assert!(h.contains("listen 127.0.0.1:8443 ssl;"));
        assert!(
            h.contains("ssl_certificate \"C:/c/cert.pem\";")
                && h.contains("ssl_certificate_key \"C:/c/key.pem\";")
        );
        assert_eq!(
            h.matches("fastcgi_pass 127.0.0.1:9001;").count(),
            2,
            "PHP served on both"
        );
    }

    #[test]
    fn renders_apache_virtual_hosts() {
        let c = SiteConfig {
            server: WebServerKind::Apache,
            service_dir: Path::new(r"C:\Devluma\services\w2"),
            site_id: "s1",
            domain: "shop.test",
            http_port: 8081,
            document_root: Path::new(r"D:\code\shop\public"),
            web_install: Path::new(r"C:\Devluma\packages\apache\2.4.69-x64"),
            php_port: Some(9001),
            https: Some(HttpsConfig {
                port: 8444,
                certificate: PathBuf::from(r"C:\c\cert.pem"),
                key: PathBuf::from(r"C:\c\key.pem"),
            }),
        };
        let a = render_config(&c);
        assert_eq!(a.matches("<VirtualHost 127.0.0.1:").count(), 2);
        assert!(a.contains("<VirtualHost 127.0.0.1:8081>") && a.contains("ServerName shop.test"));
        assert!(a.contains("DocumentRoot \"D:/code/shop/public\""));
        // Logs are absolute: relative paths would resolve against Apache's install folder.
        assert!(a.contains("ErrorLog \"C:/Devluma/services/w2/logs/site-s1.error.log\""));
        assert_eq!(
            a.matches("SetHandler \"proxy:fcgi://127.0.0.1:9001/\"")
                .count(),
            2
        );
        assert!(a.contains("Header always set X-Devluma-Site \"s1\""));
        assert!(
            a.contains("Listen 127.0.0.1:8444 https")
                && a.contains("<IfDefine !devluma_https_8444>")
        );
        assert!(a.contains("SSLCertificateKeyFile \"C:/c/key.pem\""));
        assert!(!a.contains("server {"), "no nginx syntax");
    }

    #[test]
    fn document_roots_must_be_safe_and_inside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("public");
        std::fs::create_dir(&public).unwrap();
        assert!(check_document_root(dir.path(), &public).is_ok());
        let outside = tempfile::tempdir().unwrap();
        assert!(check_document_root(dir.path(), outside.path()).is_err());
        assert!(check_document_root(dir.path(), &dir.path().join("missing")).is_err());
        let evil = dir.path().join("a$b");
        std::fs::create_dir(&evil).unwrap();
        assert!(check_document_root(dir.path(), &evil).is_err());
    }

    #[test]
    fn urls_omit_the_default_port() {
        assert_eq!(url_for("shop.test", 80), "http://shop.test/");
        assert_eq!(url_for("shop.test", 8080), "http://shop.test:8080/");
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::domain::catalog::Architecture;
    use crate::domain::operation::OperationStatus;
    use crate::persistence::operations as ops;
    use crate::persistence::services as svcs;
    use crate::services::download::DownloadOptions;
    use crate::services::install::{InstallContext, InstallManager, InstallRequest, ProcessProbe};
    use crate::services::journal::test_support::RecordingSink;
    use crate::services::supervisor::{CreateServiceRequest, ServiceStatus, StateSink};

    struct NoStates;
    impl StateSink for NoStates {
        fn emit_state(&self, _: &ServiceStatus) {}
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Both web servers would pick the same HTTPS port (8443); run them one at a time.
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// `cargo test live_site -- --ignored` (downloads ~38 MB for Nginx, ~50 MB for Apache).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_site_with_nginx_and_php() {
        live_site_with("nginx", "1.30.5").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_site_with_apache_and_php() {
        live_site_with("apache", "2.4.69").await;
    }

    /// Real web server + PHP from the bundled catalog: a PHP page served through FastCGI at a
    /// local domain, then over HTTPS with the local CA; an invalid config rejected with the
    /// previous one kept; and removal.
    async fn live_site_with(web_pkg: &str, web_version: &str) {
        let _serial = ONE_AT_A_TIME.lock().await;
        let catalog = Arc::new(crate::services::catalog::load());
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(&dir.path().join("Devluma"));
        paths.create_all().unwrap();
        let db = Arc::new(crate::persistence::test_support::db());
        let sink: Arc<dyn ProgressSink> = Arc::new(RecordingSink::default());
        let ctx = InstallContext {
            db: Arc::clone(&db),
            catalog: Arc::clone(&catalog),
            paths: paths.clone(),
            probe: Arc::new(ProcessProbe),
            sink: Arc::clone(&sink),
            download: DownloadOptions::default(),
        };
        let manager = Arc::new(InstallManager::default());
        for (pkg, version) in [(web_pkg, web_version), ("php", "8.4.26")] {
            let (started, h) = manager
                .start(
                    ctx.clone(),
                    InstallRequest {
                        package_id: pkg.into(),
                        version: version.into(),
                        architecture: Architecture::X64,
                    },
                )
                .unwrap();
            h.await.unwrap();
            let op = db.read(|c| ops::get(c, &started.operation_id)).unwrap();
            assert_eq!(
                op.status,
                OperationStatus::Succeeded,
                "{pkg}: {:?}",
                op.safe_error_message
            );
        }
        let insts = db.read(installations::list).unwrap();
        let inst = |p: &str| insts.iter().find(|i| i.package_id == p).unwrap().id.clone();
        db.write(|c| installations::set_global_default(c, &inst("php")))
            .unwrap();

        let sup = Supervisor::new(
            Arc::clone(&db),
            Arc::clone(&catalog),
            paths.clone(),
            Arc::clone(&sink),
            Arc::new(NoStates),
        );
        let _cleanup = crate::services::supervisor::StopAllOnDrop(Arc::clone(&sup));
        let web_port = free_port();
        let (op, h) = sup
            .create(CreateServiceRequest {
                installation_id: inst(web_pkg),
                name: "web".into(),
                port: web_port,
                admin_port: None,
                data_path: None,
                use_existing_data: false,
            })
            .unwrap();
        h.await.unwrap();
        assert_eq!(
            db.read(|c| ops::get(c, &op)).unwrap().status,
            OperationStatus::Succeeded
        );
        let web = db
            .read(svcs::list)
            .unwrap()
            .into_iter()
            .find(|s| s.instance_name == "web")
            .unwrap();
        let (op, h) = sup.start(&web.id).unwrap();
        h.await.unwrap();
        assert_eq!(
            db.read(|c| ops::get(c, &op)).unwrap().status,
            OperationStatus::Succeeded
        );

        // A Laravel-shaped project whose front controller prints the PHP version.
        let project_dir = paths.www.join("shop");
        std::fs::create_dir_all(project_dir.join("public")).unwrap();
        std::fs::write(project_dir.join("artisan"), "").unwrap();
        std::fs::write(
            project_dir.join("public").join("index.php"),
            "<?php echo 'hello from PHP ', PHP_VERSION;",
        )
        .unwrap();
        let project = crate::services::projects::create(&db, &paths, "Shop", "shop", &[]).unwrap();
        assert_eq!(project.project_type.as_deref(), Some("laravel"));

        let (hosts_svc, hosts_file) = crate::services::hosts::test_support::service(
            Arc::clone(&db),
            &paths,
            "127.0.0.1 localhost\r\n",
        );
        let (cert_svc, _store) =
            crate::services::certificates::test_support::service(Arc::clone(&db), &paths);
        let cert_svc = Arc::new(cert_svc);
        let sites = SiteService::new(
            Arc::clone(&db),
            paths.clone(),
            Arc::clone(&sup),
            Arc::new(hosts_svc),
            Arc::clone(&cert_svc),
            Arc::clone(&sink),
        );
        let req = SiteRequest {
            site_id: None,
            project_id: project.id.clone(),
            domain: "Shop.Test".into(),
            web_server_service_id: web.id.clone(),
            use_php: true,
            document_root: None,
            manage_hosts: true,
            expected_hosts_sha256: None,
            https: false,
        };
        let plan = sites.preview(&req).unwrap();
        assert_eq!(plan.domain.normalized, "shop.test");
        assert!(plan.document_root.ends_with("public"));
        assert!(
            plan.hosts
                .as_ref()
                .unwrap()
                .diff
                .contains("+127.0.0.1 shop.test")
        );

        let (op, h) = sites
            .apply(SiteRequest {
                site_id: Some(plan.site_id.clone()),
                ..req.clone()
            })
            .unwrap();
        h.await.unwrap();
        let op = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
        assert!(
            std::fs::read_to_string(&hosts_file)
                .unwrap()
                .contains("127.0.0.1 shop.test")
        );

        let check = sites.check_site(&plan.site_id);
        if check.status != Some(200) {
            // Show the web server's own explanation in the failure.
            let logs = sup.service_dir(&web.id).join("logs");
            let text: String = std::fs::read_dir(&logs)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains("error"))
                .map(|e| {
                    let t = std::fs::read_to_string(e.path()).unwrap_or_default();
                    format!(
                        "--- {}\n{}",
                        e.path().display(),
                        t.lines().rev().take(15).collect::<Vec<_>>().join("\n")
                    )
                })
                .collect();
            panic!("{}\n{text}", check.message);
        }
        let body = reqwest::Client::builder()
            .resolve(
                "shop.test",
                std::net::SocketAddr::from(([127, 0, 0, 1], web_port)),
            )
            .build()
            .unwrap()
            .get(format!("http://shop.test:{web_port}/"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "hello from PHP 8.4.26");

        // HTTPS with a certificate from the local CA, checked by a TLS client that trusts only
        // that CA (so the name-constrained chain must validate for real).
        let (op, h) = sites
            .apply(SiteRequest {
                site_id: Some(plan.site_id.clone()),
                https: true,
                ..req.clone()
            })
            .unwrap();
        h.await.unwrap();
        let op = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
        let site_row = db.read(|c| sites::get(c, &plan.site_id)).unwrap();
        assert!(site_row.ssl_enabled);
        let https_port = u16::try_from(site_row.https_port.unwrap()).unwrap();
        let ca = cert_svc.ca().unwrap().unwrap();
        assert!(
            !cert_svc.ca_trusted_now(&ca),
            "HTTPS setup never trusts the CA by itself"
        );
        let ca_pem = std::fs::read(&ca.certificate_path).unwrap();
        let tls = reqwest::Client::builder()
            .tls_certs_only([reqwest::Certificate::from_pem(&ca_pem).unwrap()])
            .resolve(
                "shop.test",
                std::net::SocketAddr::from(([127, 0, 0, 1], https_port)),
            )
            .build()
            .unwrap();
        let body = tls
            .get(format!("https://shop.test:{https_port}/"))
            .send()
            .await
            .expect("TLS handshake with the local CA's certificate")
            .text()
            .await
            .unwrap();
        assert_eq!(body, "hello from PHP 8.4.26");
        let bound = db
            .read(|c| crate::persistence::certificates::for_site(c, &plan.site_id))
            .unwrap()
            .unwrap();
        assert!(
            cert_svc
                .delete(&bound.id)
                .unwrap_err()
                .message
                .contains("Turn off HTTPS"),
            "a certificate in use can't be deleted"
        );

        // An invalid config is rejected and the working one is kept (and still serving).
        let good_config = std::fs::read_to_string(&plan.config_path).unwrap();
        std::fs::write(
            sup.service_dir(&web.id).join("conf").join("custom.conf"),
            "this is not nginx syntax\n",
        )
        .unwrap();
        let (op, h) = sites
            .apply(SiteRequest {
                site_id: Some(plan.site_id.clone()),
                use_php: false,
                ..req.clone()
            })
            .unwrap();
        h.await.unwrap();
        let op = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            op.error_code.as_deref(),
            Some("CONFIG_INVALID"),
            "{:?}",
            op.safe_error_message
        );
        assert!(
            op.safe_error_message
                .unwrap()
                .contains("previous configuration was kept")
        );
        assert_eq!(
            std::fs::read_to_string(&plan.config_path).unwrap(),
            good_config
        );
        assert_eq!(sites.check_site(&plan.site_id).status, Some(200));
        std::fs::write(
            sup.service_dir(&web.id).join("conf").join("custom.conf"),
            "",
        )
        .unwrap();

        // Removal takes out the hosts entry and the config.
        let (op, h) = sites.remove(&plan.site_id).unwrap();
        h.await.unwrap();
        assert_eq!(
            db.read(|c| ops::get(c, &op)).unwrap().status,
            OperationStatus::Succeeded
        );
        assert_eq!(
            std::fs::read_to_string(&hosts_file).unwrap(),
            "127.0.0.1 localhost\r\n"
        );
        assert!(!Path::new(&plan.config_path).exists());
        cert_svc.delete(&bound.id).unwrap();
        cert_svc.delete(&ca.id).unwrap();

        // Stop everything Devluma started.
        let all = db.read(svcs::list).unwrap();
        for s in all.iter().filter(|s| s.observed_state.is_live()) {
            let (op, h) = sup.stop(&s.id).unwrap();
            h.await.unwrap();
            assert_eq!(
                db.read(|c| ops::get(c, &op)).unwrap().status,
                OperationStatus::Succeeded
            );
        }
    }
}
