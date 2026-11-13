//! App installer (ADR 0020): a new project from WordPress, Laravel or Symfony, in one
//! journaled operation (`create_app_project`):
//!
//! 1. download what's missing through the normal installer (WordPress, Composer);
//! 2. turn on the PHP extensions the app needs (config service: backup, check, rollback);
//! 3. build the app in `staging\<operation>\app`: copy the verified WordPress release, or run
//!    `composer create-project` (the app's own post-create scripts run then, which is what
//!    the user asked for);
//! 4. WordPress: create an empty database and write `wp-config.php`;
//! 5. move the folder into `www\<folder>` (one rename), register the project;
//! 6. optionally create its site (its own `apply_site` operation).
//!
//! Until step 5 nothing exists outside staging. A failure removes staging, the database it
//! created and, if registration fails after the move, the folder it moved in; nothing that
//! existed before is touched (the target must be missing or an empty folder).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::composer;
use super::config::ConfigService;
use super::databases::DatabaseService;
use super::install::{ExecutableProbe, InstallContext, InstallManager, InstallRequest};
use super::journal::{Journal, ProgressSink};
use super::projects::{self, SiteOptions};
use super::runtime::{self, Selection};
use super::sites::SiteService;
use super::tasks::Tasks;
use crate::domain::adapter::AdapterId;
use crate::domain::catalog::{Catalog, VersionManifest};
use crate::error::{ErrorCode, IpcError};
use crate::persistence::operations::{EventLevel, NewOperation};
use crate::persistence::{Database, new_id, services};
use crate::state::DataPaths;

pub const OPERATION_TYPE: &str = "create_app_project";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum App {
    Wordpress,
    Laravel,
    Symfony,
}

impl App {
    pub const ALL: [App; 3] = [App::Wordpress, App::Laravel, App::Symfony];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wordpress => "WordPress",
            Self::Laravel => "Laravel",
            Self::Symfony => "Symfony",
        }
    }

    /// Composer's `create-project` package (compiled, never from input).
    fn composer_package(self) -> Option<&'static str> {
        match self {
            Self::Wordpress => None,
            Self::Laravel => Some("laravel/laravel"),
            Self::Symfony => Some("symfony/skeleton"),
        }
    }

    /// Extensions the app (and Composer, for Composer apps) needs from PHP. All ship as
    /// DLLs with the official Windows builds.
    pub fn php_extensions(self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = match self {
            // WordPress's required mysqli plus what Site Health asks for.
            Self::Wordpress => vec!["mysqli", "openssl", "curl", "mbstring", "fileinfo", "zip"],
            // Laravel's default database is SQLite; its post-create script migrates it.
            Self::Laravel => [composer::EXTENSIONS, &["fileinfo", "pdo_sqlite"]].concat(),
            Self::Symfony => composer::EXTENSIONS.to_vec(),
        };
        v.dedup();
        v
    }

    /// Lowest PHP minor the app's current releases accept (Composer picks the newest app
    /// release that fits the PHP version it runs with).
    fn min_php(self) -> (u64, u64) {
        match self {
            Self::Wordpress => (7, 4),
            Self::Laravel | Self::Symfony => (8, 2),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppProjectRequest {
    pub name: String,
    pub folder: String,
    pub app: App,
    #[serde(default)]
    pub service_ids: Vec<String>,
    /// Create a site for the new project.
    #[serde(default)]
    pub site: Option<SiteOptions>,
    /// WordPress: database server to create the project's database on (must be running).
    #[serde(default)]
    pub database_service_id: Option<String>,
    /// Which release: a catalog version for WordPress ("7.1.4"), a release line for
    /// Composer apps ("13.*", "7.4.*"). `None` = the newest that fits.
    #[serde(default)]
    pub version: Option<String>,
    /// Install into this existing project (its folder must be empty) instead of a new one;
    /// `name` and `folder` are then ignored.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// One release a user can pick in the installer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppVersion {
    /// What goes into the request's `version`.
    pub value: String,
    /// "Laravel 13.x", "Symfony 7.4 LTS", "WordPress 7.1.4".
    pub label: String,
    /// The release that's installed for this choice right now.
    pub latest: String,
    /// Lowest PHP it accepts ("8.3"), when known.
    pub min_php: Option<String>,
    /// The default PHP is new enough.
    pub supported: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppVersions {
    pub app: App,
    /// Newest first.
    pub versions: Vec<AppVersion>,
    /// Where the list comes from ("Devluma's catalog", "packagist.org").
    pub source: String,
    /// Why no list could be read (the newest compatible release is used then).
    pub unavailable: Option<String>,
}

/// What creating the project will do, for the form. Changes nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPlan {
    pub app: App,
    pub label: String,
    pub target_path: String,
    pub php_version: Option<String>,
    /// Downloads that happen first, e.g. "WordPress 7.1.4 (36 MB)".
    pub downloads: Vec<String>,
    /// PHP extensions turned on in php.ini.
    pub enable_extensions: Vec<String>,
    /// WordPress: the database that is created.
    pub database_name: Option<String>,
    /// Plain-language steps, in order.
    pub steps: Vec<String>,
    /// Why it can't run now.
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStarted {
    pub operation_id: String,
    /// The project's id once it's registered (assigned up front).
    pub project_id: String,
}

/// A catalog version to download first.
#[derive(Clone)]
struct Download {
    package_id: String,
    version: VersionManifest,
    label: String,
}

/// Everything the operation needs, resolved before it starts.
#[derive(Clone)]
struct Resolved {
    req: AppProjectRequest,
    target: PathBuf,
    php: Selection,
    downloads: Vec<Download>,
    enable_extensions: Vec<String>,
    database: Option<(String, String, u16)>,
    /// The project to install into, when it exists already.
    existing: Option<crate::persistence::projects::Project>,
    /// Composer apps: the release line passed to `create-project` ("13.*").
    composer_version: Option<String>,
    /// WordPress: the catalog release downloaded into this project (pinned SHA-256).
    wordpress: Option<VersionManifest>,
}

/// Whether two paths are on the same drive (a rename between them is possible).
fn same_volume(a: &Path, b: &Path) -> bool {
    let prefix = |p: &Path| match p.components().next() {
        Some(std::path::Component::Prefix(x)) => {
            Some(x.as_os_str().to_string_lossy().to_ascii_lowercase())
        }
        _ => None,
    };
    prefix(a) == prefix(b)
}

/// "8.3" from a PHP constraint such as `^8.3`, `>=8.4` or `^8.2|^8.3` (the lowest).
pub fn min_php_of(constraint: &str) -> Option<(u64, u64)> {
    let re = regex::Regex::new(r"(\d+)\.(\d+)").ok()?;
    constraint
        .split('|')
        .filter_map(|alt| {
            re.captures(alt)
                .map(|c| (c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0)))
        })
        .min()
}

/// A release line Composer accepts as the version argument: "13.*" or "7.4.*".
fn valid_line(spec: &str) -> bool {
    regex::Regex::new(r"^\d{1,3}(\.\d{1,3})?\.\*$")
        .map(|re| re.is_match(spec))
        .unwrap_or(false)
}

/// Release lines from packagist.org's `p2/<package>.json` (Composer 2 "minified" format:
/// each version repeats only the fields that changed). Laravel lines are majors ("13.x"),
/// Symfony lines minors ("7.4", with LTS marked). Newest first, at most `max`.
pub fn release_lines(json: &str, app: App, php: (u64, u64), max: usize) -> Vec<AppVersion> {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(pkg) = app.composer_package() else {
        return Vec::new();
    };
    let Some(list) = doc["packages"][pkg].as_array() else {
        return Vec::new();
    };
    let mut require_php: Option<String> = None;
    let mut lines: Vec<AppVersion> = Vec::new();
    for entry in list {
        // Minified: `require` carries over from the previous entry unless replaced.
        match &entry["require"] {
            serde_json::Value::Object(r) => {
                require_php = r.get("php").and_then(|v| v.as_str()).map(str::to_owned);
            }
            serde_json::Value::String(s) if s == "__unset" => require_php = None,
            _ => {}
        }
        let Some(norm) = entry["version_normalized"].as_str() else {
            continue;
        };
        let parts: Vec<u64> = norm.split('.').map_while(|p| p.parse().ok()).collect();
        if parts.len() < 3 || norm.contains('-') {
            continue; // not a stable release
        }
        let (value, label) = match app {
            App::Symfony => (
                format!("{}.{}.*", parts[0], parts[1]),
                format!(
                    "Symfony {}.{}{}",
                    parts[0],
                    parts[1],
                    if parts[1] == 4 { " LTS" } else { "" }
                ),
            ),
            _ => (
                format!("{}.*", parts[0]),
                format!("{} {}.x", app.label(), parts[0]),
            ),
        };
        if lines.iter().any(|l| l.value == value) {
            continue; // the newest of each line comes first in the file
        }
        let min = require_php.as_deref().and_then(min_php_of);
        lines.push(AppVersion {
            value,
            label,
            latest: entry["version"]
                .as_str()
                .unwrap_or(norm)
                .trim_start_matches('v')
                .to_owned(),
            min_php: min.map(|(a, b)| format!("{a}.{b}")),
            supported: min.is_none_or(|m| php >= m),
        });
        if lines.len() == max {
            break;
        }
    }
    lines
}

pub struct AppsService {
    db: Arc<Database>,
    catalog: Arc<Catalog>,
    paths: DataPaths,
    probe: Arc<dyn ExecutableProbe>,
    sink: Arc<dyn ProgressSink>,
    config: Arc<ConfigService>,
    databases: Arc<DatabaseService>,
    sites: Arc<SiteService>,
    installs: Arc<InstallManager>,
    tasks: Arc<Tasks>,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, msg, false)
}

fn failed(code: ErrorCode, msg: impl Into<String>) -> IpcError {
    IpcError::new(code, msg, true)
}

/// "clients\My Blog" → "my_blog": the last folder name, as a database name.
pub fn database_name(folder: &str) -> String {
    let last = folder
        .trim()
        .trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default();
    let mut name: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    let name: String = name.trim_matches('_').chars().take(60).collect();
    if name.is_empty() {
        "wordpress".into()
    } else {
        name
    }
}

/// The newest current catalog version of `package_id` this computer can run.
fn current_version<'c>(catalog: &'c Catalog, package_id: &str) -> Option<&'c VersionManifest> {
    let pkg = catalog.package(package_id)?;
    pkg.versions
        .iter()
        .filter(|v| v.superseded_by.is_none() && v.architecture.runs_on_host())
        .max_by_key(|v| composer::version_key(&v.version))
}

fn php_minor(version: &str) -> (u64, u64) {
    let k = composer::version_key(version);
    (
        k.first().copied().unwrap_or(0),
        k.get(1).copied().unwrap_or(0),
    )
}

/// Random text for WordPress's keys and salts (no quotes or backslashes).
fn random_phrase() -> Result<String, IpcError> {
    use windows::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    const CHARS: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*()-_[]{}<>~+=,.;:/?|";
    let mut buf = [0u8; 64];
    unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(|_| IpcError::internal())?;
    Ok(buf
        .iter()
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect())
}

/// `wp-config.php` from WordPress's own sample: database settings and fresh keys. `None`
/// when the sample doesn't have the expected placeholders (WordPress's setup then asks).
pub fn wp_config(
    sample: &str,
    database: &str,
    port: u16,
    mut phrase: impl FnMut() -> Result<String, IpcError>,
) -> Result<Option<String>, IpcError> {
    const SALT: &str = "'put your unique phrase here'";
    let placeholders = [
        "'database_name_here'",
        "'username_here'",
        "'password_here'",
        "'localhost'",
    ];
    if placeholders.iter().any(|p| !sample.contains(p)) || sample.matches(SALT).count() != 8 {
        return Ok(None);
    }
    let mut text = sample
        .replacen("'database_name_here'", &format!("'{database}'"), 1)
        .replacen("'username_here'", "'root'", 1)
        .replacen("'password_here'", "''", 1)
        .replacen("'localhost'", &format!("'127.0.0.1:{port}'"), 1);
    while text.contains(SALT) {
        text = text.replacen(SALT, &format!("'{}'", phrase()?), 1);
    }
    Ok(Some(text))
}

impl AppsService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Database>,
        catalog: Arc<Catalog>,
        paths: DataPaths,
        probe: Arc<dyn ExecutableProbe>,
        sink: Arc<dyn ProgressSink>,
        config: Arc<ConfigService>,
        databases: Arc<DatabaseService>,
        sites: Arc<SiteService>,
        installs: Arc<InstallManager>,
        tasks: Arc<Tasks>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            catalog,
            paths,
            probe,
            sink,
            config,
            databases,
            sites,
            installs,
            tasks,
        })
    }

    fn resolve(&self, req: &AppProjectRequest) -> Result<Resolved, IpcError> {
        let app = req.app;
        let existing = match &req.project_id {
            Some(id) => Some(
                self.db
                    .read(|c| crate::persistence::projects::get(c, id))
                    .map_err(|_| invalid("That project no longer exists."))?,
            ),
            None => None,
        };
        let target = match &existing {
            Some(p) => PathBuf::from(&p.root_path),
            None => {
                if req.name.trim().is_empty() {
                    return Err(invalid("Enter a project name."));
                }
                projects::project_dir(&req.folder, &self.paths)?
            }
        };
        if target.is_file() {
            return Err(invalid("That path is a file, not a folder."));
        }
        if existing.is_some() && !target.is_dir() {
            return Err(invalid(format!(
                "The project's folder {} no longer exists.",
                target.display()
            )));
        }
        if target.is_dir()
            && std::fs::read_dir(&target)
                .map(|mut d| d.next().is_some())
                .unwrap_or(true)
        {
            return Err(invalid(format!(
                "{} isn't empty. {} is installed into a new or empty folder only, so nothing in it is overwritten.",
                target.display(),
                app.label()
            )));
        }
        // The app is built next to Devluma's data and moved in with one rename.
        if !same_volume(&target, &self.paths.staging) {
            return Err(invalid(format!(
                "Apps can only be installed into folders on the same drive as {}.",
                self.paths.root.display()
            )));
        }
        if self.tasks.busy(&target.display().to_string()) {
            return Err(invalid(
                "A project is already being created in that folder.",
            ));
        }
        let php = self
            .db
            .read(|c| runtime::effective(c, req.project_id.as_deref(), "php"))?
            .ok_or_else(|| {
                invalid(format!(
                    "{} needs PHP. Install a PHP version and make it the default on the Runtimes & Packages page.",
                    app.label()
                ))
            })?;
        let (min_major, min_minor) = app.min_php();
        if php_minor(&php.version) < (min_major, min_minor) {
            return Err(invalid(format!(
                "{} needs PHP {min_major}.{min_minor} or newer; this project uses PHP {}.",
                app.label(),
                php.version
            )));
        }

        // The chosen release. WordPress is downloaded into this project only (never
        // installed as a package); Composer apps name a release line for create-project.
        let requested = req
            .version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let mut composer_version = None;
        let mut wordpress = None;
        let mut downloads = Vec::new();
        if app == App::Wordpress {
            let v = match requested {
                Some(v) => self.catalog.package("wordpress").and_then(|p| {
                    p.versions
                        .iter()
                        .find(|x| x.version == v && x.superseded_by.is_none())
                }),
                None => current_version(&self.catalog, "wordpress"),
            }
            .ok_or_else(|| {
                invalid(match requested {
                    Some(v) => format!("WordPress {v} isn't offered."),
                    None => "This catalog doesn't offer WordPress.".into(),
                })
            })?;
            wordpress = Some(v.clone());
        } else {
            if let Some(v) = requested {
                if !valid_line(v) {
                    return Err(invalid(format!(
                        "\"{v}\" isn't a release line such as 13.* or 7.4.*."
                    )));
                }
                composer_version = Some(v.to_owned());
            }
            // Composer is a shared tool: installed once, when it's missing.
            if self
                .db
                .read(|c| composer::newest_installed(c, composer::PACKAGE_ID))?
                .is_none()
            {
                let v = current_version(&self.catalog, composer::PACKAGE_ID)
                    .ok_or_else(|| invalid("This catalog doesn't offer Composer."))?;
                downloads.push(Download {
                    package_id: composer::PACKAGE_ID.to_owned(),
                    version: v.clone(),
                    label: format!(
                        "Composer {} ({} MB)",
                        v.version,
                        v.source.size_bytes.div_ceil(1_048_576)
                    ),
                });
            }
        }

        let enable_extensions = composer::missing_extensions(
            &*self.probe,
            Path::new(&php.install_path),
            &app.php_extensions(),
        )
        .map_err(|e| invalid(format!("Couldn't check PHP {}: {e}", php.version)))?;

        let database = match (&req.database_service_id, app) {
            (Some(id), App::Wordpress) => {
                let svc = self
                    .db
                    .read(|c| services::get(c, id))
                    .map_err(|_| invalid("That database service no longer exists."))?;
                if svc.lifecycle_adapter != AdapterId::Mariadb.as_str() {
                    return Err(invalid("WordPress needs a MariaDB database service."));
                }
                if svc.observed_state.as_str() != "running" {
                    return Err(invalid(format!(
                        "Start {} first, so the WordPress database can be created.",
                        svc.instance_name
                    )));
                }
                let name = database_name(&target.display().to_string());
                let existing = self
                    .databases
                    .databases(id)
                    .map_err(|f| IpcError::new(f.code, f.message, true))?;
                if existing.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
                    return Err(invalid(format!(
                        "{} already has a database named {name}. Choose another folder name.",
                        svc.instance_name
                    )));
                }
                let port = svc
                    .port
                    .and_then(|p| u16::try_from(p).ok())
                    .ok_or_else(|| invalid("That database service has no port."))?;
                Some((id.clone(), name, port))
            }
            _ => None,
        };
        if let Some(site) = &req.site {
            let ok = self
                .db
                .read(|c| services::get(c, &site.web_server_service_id))
                .ok()
                .and_then(|s| AdapterId::parse(&s.lifecycle_adapter))
                .and_then(AdapterId::service_role)
                == Some(crate::domain::adapter::ServiceRole::WebServer);
            if !ok {
                return Err(invalid("Choose a web server for the site."));
            }
        }
        Ok(Resolved {
            req: req.clone(),
            target,
            php,
            downloads,
            enable_extensions,
            database,
            existing,
            composer_version,
            wordpress,
        })
    }

    fn describe(r: &Resolved) -> AppPlan {
        let app = r.req.app;
        let mut steps: Vec<String> = r
            .downloads
            .iter()
            .map(|d| format!("Download and verify {}.", d.label))
            .collect();
        if !r.enable_extensions.is_empty() {
            steps.push(format!(
                "Turn on {} in PHP {}'s php.ini (backed up first).",
                r.enable_extensions.join(", "),
                r.php.version
            ));
        }
        steps.push(match (app.composer_package(), &r.wordpress) {
            (Some(pkg), _) => format!(
                "Run composer create-project {pkg}{} with PHP {} (downloads the app's packages from packagist.org and runs its setup scripts).",
                r.composer_version
                    .as_deref()
                    .map(|v| format!(" {v}"))
                    .unwrap_or_default(),
                r.php.version
            ),
            (None, Some(v)) => format!(
                "Download WordPress {} ({} MB) from wordpress.org, check its SHA-256 and unpack it into the project folder only.",
                v.version,
                v.source.size_bytes.div_ceil(1_048_576)
            ),
            (None, None) => "Unpack WordPress into the project folder.".into(),
        });
        if let Some((_, name, _)) = &r.database {
            steps.push(format!(
                "Create the database {name} and write wp-config.php (root, no password, new keys)."
            ));
        }
        steps.push(match &r.existing {
            Some(p) => format!("Install it into {} ({}).", p.name, r.target.display()),
            None => format!("Add the project at {}.", r.target.display()),
        });
        if let Some(site) = &r.req.site {
            steps.push(format!(
                "Create the site {} (Windows may ask for permission to add it to the hosts file).",
                site.domain.trim()
            ));
        }
        AppPlan {
            app,
            label: app.label().into(),
            target_path: r.target.display().to_string(),
            php_version: Some(r.php.version.clone()),
            downloads: r
                .downloads
                .iter()
                .map(|d| d.label.clone())
                .chain(r.wordpress.iter().map(|v| {
                    format!(
                        "WordPress {} ({} MB)",
                        v.version,
                        v.source.size_bytes.div_ceil(1_048_576)
                    )
                }))
                .collect(),
            enable_extensions: r.enable_extensions.clone(),
            database_name: r.database.as_ref().map(|d| d.1.clone()),
            steps,
            blocked: None,
        }
    }

    /// The PHP a project (or a new project: the default) uses, as (major, minor).
    pub fn php_for(&self, project_id: Option<&str>) -> Option<(u64, u64)> {
        self.db
            .read(|c| runtime::effective(c, project_id, "php"))
            .ok()
            .flatten()
            .map(|s| php_minor(&s.version))
    }

    /// Releases to choose from: WordPress from Devluma's catalog (pinned, reviewed), Laravel
    /// and Symfony release lines from packagist.org. When packagist can't be reached, the
    /// list is empty and Composer picks the newest release that fits the PHP version.
    pub async fn versions(&self, app: App, php: Option<(u64, u64)>) -> AppVersions {
        let php = php.unwrap_or((0, 0));
        let (min_major, min_minor) = app.min_php();
        let Some(pkg) = app.composer_package() else {
            let versions = self
                .catalog
                .package("wordpress")
                .map(|p| {
                    let mut v: Vec<_> = p
                        .versions
                        .iter()
                        .filter(|v| v.superseded_by.is_none() && v.architecture.runs_on_host())
                        .collect();
                    v.sort_by_key(|v| std::cmp::Reverse(composer::version_key(&v.version)));
                    v.into_iter()
                        .map(|v| AppVersion {
                            value: v.version.clone(),
                            label: format!("WordPress {}", v.version),
                            latest: v.version.clone(),
                            min_php: Some(format!("{min_major}.{min_minor}")),
                            supported: php >= (min_major, min_minor),
                        })
                        .collect()
                })
                .unwrap_or_default();
            return AppVersions {
                app,
                versions,
                source: "Devluma's catalog (each release checked against its SHA-256)".into(),
                unavailable: None,
            };
        };
        let url = format!("https://repo.packagist.org/p2/{pkg}.json");
        let fetched = async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent(concat!("Devluma/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.to_string())?;
            let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("packagist.org answered {}", resp.status()));
            }
            resp.text().await.map_err(|e| e.to_string())
        }
        .await;
        match fetched {
            Ok(json) => {
                let versions = release_lines(&json, app, php, 5);
                let unavailable = versions
                    .is_empty()
                    .then(|| "packagist.org didn't list any releases.".to_owned());
                AppVersions {
                    app,
                    versions,
                    source: "packagist.org".into(),
                    unavailable,
                }
            }
            Err(e) => AppVersions {
                app,
                versions: Vec::new(),
                source: "packagist.org".into(),
                unavailable: Some(format!(
                    "Couldn't read the releases from packagist.org ({e}). Composer will install the newest release that fits your PHP version."
                )),
            },
        }
    }

    /// What creating the project would do, or why it can't. Changes nothing.
    pub fn preview(&self, req: &AppProjectRequest) -> AppPlan {
        match self.resolve(req) {
            Ok(r) => Self::describe(&r),
            Err(e) => AppPlan {
                app: req.app,
                label: req.app.label().into(),
                target_path: projects::project_dir(&req.folder, &self.paths)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                php_version: None,
                downloads: Vec::new(),
                enable_extensions: Vec::new(),
                database_name: None,
                steps: Vec::new(),
                blocked: Some(e.message),
            },
        }
    }

    /// Starts the operation. `ctx` is used for downloads.
    pub fn start(
        self: &Arc<Self>,
        ctx: InstallContext,
        req: &AppProjectRequest,
    ) -> Result<AppStarted, IpcError> {
        let r = self.resolve(req)?;
        let project_id = r.existing.as_ref().map_or_else(new_id, |p| p.id.clone());
        let journal = Journal::new(&self.db, self.sink.as_ref());
        let op = journal.start(&NewOperation {
            operation_type: OPERATION_TYPE,
            target_type: Some("project"),
            target_id: Some(&project_id),
        })?;
        let Some(cancel) = self.tasks.register(&op.id, &r.target.display().to_string()) else {
            let _ = journal.fail(&op.id, ErrorCode::ValidationError, "Already running.");
            return Err(invalid(
                "A project is already being created in that folder.",
            ));
        };
        let (me, op_id, pid) = (Arc::clone(self), op.id.clone(), project_id.clone());
        tauri::async_runtime::spawn(async move {
            let journal = Journal::new(&me.db, me.sink.as_ref());
            let _ = journal.running(&op_id);
            let mut result = me.download_all(&ctx, &op_id, &r, &cancel).await;
            if result.is_ok() {
                let (me2, op2, pid2, cancel2) = (
                    Arc::clone(&me),
                    op_id.clone(),
                    pid.clone(),
                    Arc::clone(&cancel),
                );
                result = tauri::async_runtime::spawn_blocking(move || {
                    me2.build(&op2, &pid2, &r, &cancel2)
                })
                .await
                .unwrap_or_else(|_| Err(IpcError::internal()));
            }
            let staging = me.paths.staging.join(&op_id);
            if staging.exists() {
                let _ = std::fs::remove_dir_all(&staging);
            }
            let _ = match result {
                Ok(()) => journal.succeed(&op_id).map(|_| ()),
                Err(_) if cancel.load(Ordering::SeqCst) => journal.cancel(&op_id).map(|_| ()),
                Err(e) => journal.fail(&op_id, e.code, &e.message).map(|_| ()),
            };
            me.tasks.finish(&op_id);
        });
        Ok(AppStarted {
            operation_id: op.id,
            project_id,
        })
    }

    async fn download_all(
        &self,
        ctx: &InstallContext,
        op_id: &str,
        r: &Resolved,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), IpcError> {
        let journal = Journal::new(&self.db, self.sink.as_ref());
        for d in &r.downloads {
            journal.progress(op_id, &format!("Downloading {}", d.label), None, None)?;
            let (started, mut handle) = self.installs.start(
                ctx.clone(),
                InstallRequest {
                    package_id: d.package_id.clone(),
                    version: d.version.version.clone(),
                    architecture: d.version.architecture,
                },
            )?;
            loop {
                tokio::select! {
                    _ = &mut handle => break,
                    () = tokio::time::sleep(Duration::from_millis(200)) => {
                        if cancel.load(Ordering::SeqCst) {
                            self.installs.cancel(&started.operation_id);
                        }
                    }
                }
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(invalid("Cancelled."));
            }
            let ok = self
                .db
                .read(|c| composer::newest_installed(c, &d.package_id))?
                .is_some();
            if !ok {
                return Err(failed(
                    ErrorCode::NetworkError,
                    format!(
                        "{} couldn't be installed; the install operation says why.",
                        d.label
                    ),
                ));
            }
        }
        if let Some(v) = &r.wordpress {
            self.fetch_wordpress(ctx, op_id, v, cancel).await?;
        }
        Ok(())
    }

    /// Downloads a WordPress release for this project only: into staging, checked against
    /// the catalog's SHA-256, unpacked safely into `staging\<op>\app` and its version read
    /// from `wp-includes/version.php`. Nothing is installed in `bin`.
    async fn fetch_wordpress(
        &self,
        ctx: &InstallContext,
        op_id: &str,
        v: &VersionManifest,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(), IpcError> {
        let journal = Journal::new(&self.db, self.sink.as_ref());
        let pkg = self
            .catalog
            .package("wordpress")
            .ok_or_else(|| invalid("This catalog doesn't offer WordPress."))?;
        let staging = self.paths.staging.join(op_id);
        std::fs::create_dir_all(&staging)
            .map_err(|_| failed(ErrorCode::Internal, "Couldn't prepare the staging folder."))?;
        let archive_path = staging.join("wordpress.zip");
        let label = format!("Downloading WordPress {}", v.version);
        journal.progress(op_id, &label, None, None)?;
        let token = tokio_util::sync::CancellationToken::new();
        let mut last = std::time::Instant::now() - Duration::from_secs(1);
        let download = super::download::download(
            &v.source.url,
            &pkg.manifest.allowed_hosts,
            v.source.size_bytes,
            &archive_path,
            &ctx.download,
            &token,
            |received, total| {
                if last.elapsed() >= Duration::from_millis(250) || received == total {
                    last = std::time::Instant::now();
                    let _ =
                        journal.progress(op_id, &label, Some(received as i64), Some(total as i64));
                }
            },
        );
        tokio::pin!(download);
        let downloaded = loop {
            tokio::select! {
                r = &mut download => break r,
                () = tokio::time::sleep(Duration::from_millis(200)) => {
                    if cancel.load(Ordering::SeqCst) {
                        token.cancel();
                    }
                }
            }
        };
        let downloaded = match downloaded {
            Ok(d) => d,
            Err(super::download::DownloadError::Cancelled) => return Err(invalid("Cancelled.")),
            Err(e) => {
                return Err(failed(
                    ErrorCode::NetworkError,
                    format!("Downloading WordPress failed: {e}."),
                ));
            }
        };
        journal.progress(op_id, "Checking WordPress's SHA-256", None, None)?;
        if downloaded.sha256 != v.integrity.sha256 {
            return Err(failed(
                ErrorCode::IntegrityError,
                format!(
                    "Checksum mismatch: expected SHA-256 {}, got {}. The download was discarded.",
                    v.integrity.sha256, downloaded.sha256
                ),
            ));
        }
        journal.progress(op_id, "Unpacking WordPress", None, None)?;
        let app_dir = staging.join("app");
        let limits = crate::security::archive::ExtractLimits::for_archive(
            v.source.size_bytes,
            v.source.strip_components,
        );
        let (zip, dest, flag) = (archive_path.clone(), app_dir.clone(), Arc::clone(cancel));
        tauri::async_runtime::spawn_blocking(move || {
            crate::security::archive::extract_zip(&zip, &dest, limits, &flag)
        })
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(|e| match e {
            crate::security::archive::ExtractError::Cancelled => invalid("Cancelled."),
            other => failed(
                ErrorCode::IntegrityError,
                format!("The WordPress archive was rejected: {other}."),
            ),
        })?;
        let _ = std::fs::remove_file(&archive_path);
        let text =
            std::fs::read_to_string(app_dir.join(r"wp-includes\version.php")).unwrap_or_default();
        let expected = v
            .executables
            .first()
            .map_or(v.version.as_str(), |e| e.expected_version.as_str());
        match AdapterId::Wordpress.parse_version(&text) {
            Some(found) if found == expected => Ok(()),
            found => Err(failed(
                ErrorCode::IntegrityError,
                format!(
                    "The download says it's WordPress {}, expected {expected}.",
                    found.as_deref().unwrap_or("(unknown)")
                ),
            )),
        }
    }

    fn build(
        &self,
        op_id: &str,
        project_id: &str,
        r: &Resolved,
        cancel: &AtomicBool,
    ) -> Result<(), IpcError> {
        let journal = Journal::new(&self.db, self.sink.as_ref());
        let app = r.req.app;
        let check = || {
            if cancel.load(Ordering::SeqCst) {
                Err(invalid("Cancelled."))
            } else {
                Ok(())
            }
        };

        if !r.enable_extensions.is_empty() {
            journal.progress(
                op_id,
                &format!(
                    "Turning on PHP extensions: {}",
                    r.enable_extensions.join(", ")
                ),
                None,
                None,
            )?;
            let restart = composer::enable_extensions(
                &self.config,
                &*self.probe,
                &r.php,
                &r.enable_extensions,
            )?;
            // Running php-cgi backends of this PHP keep the old php.ini until they restart;
            // the new site would otherwise run without the extensions.
            for hint in restart {
                journal.progress(op_id, &format!("Restarting {}", hint.name), None, None)?;
                let sup = self.sites.supervisor();
                if let Ok((_, handle)) = sup.restart(&hint.service_id) {
                    let _ = tauri::async_runtime::block_on(handle);
                }
            }
        }
        check()?;

        let staging = self.paths.staging.join(op_id);
        let app_dir = staging.join("app");
        std::fs::create_dir_all(&staging)
            .map_err(|_| failed(ErrorCode::Internal, "Couldn't prepare the staging folder."))?;
        match app.composer_package() {
            // Already downloaded and unpacked into staging by `fetch_wordpress`.
            None => {
                if !app_dir.join("wp-load.php").is_file() {
                    return Err(failed(
                        ErrorCode::Internal,
                        "WordPress wasn't unpacked; nothing was added.",
                    ));
                }
            }
            Some(pkg) => {
                let package = self
                    .db
                    .read(|c| composer::newest_installed(c, composer::PACKAGE_ID))?
                    .ok_or_else(|| invalid("Composer isn't installed."))?;
                journal.progress(
                    op_id,
                    format!(
                        "composer create-project {pkg} {}",
                        r.composer_version.as_deref().unwrap_or("")
                    )
                    .trim_end(),
                    None,
                    None,
                )?;
                let mut args: Vec<std::ffi::OsString> =
                    vec!["create-project".into(), pkg.into(), "app".into()];
                if let Some(v) = &r.composer_version {
                    args.push(v.into());
                }
                args.extend(["--prefer-dist", "--no-interaction", "--no-ansi"].map(Into::into));
                let log = composer::log_path(&self.paths, op_id);
                let result = composer::run(
                    composer::Run {
                        php_exe: &Path::new(&r.php.install_path).join("php.exe"),
                        phar: &Path::new(&package.install_path).join(composer::PHAR),
                        args,
                        cwd: &staging,
                        log: &log,
                        timeout: Duration::from_secs(30 * 60),
                        cancel,
                        env: composer::env(&self.paths),
                    },
                    |line| {
                        let _ = journal.progress(op_id, line, None, None);
                    },
                );
                match result {
                    Ok(()) => {}
                    Err(composer::RunError::Cancelled) => return Err(invalid("Cancelled.")),
                    Err(composer::RunError::Failed(msg)) => {
                        return Err(failed(ErrorCode::ProcessExited, msg));
                    }
                }
                if !app_dir.join("composer.json").is_file() {
                    return Err(failed(
                        ErrorCode::ProcessExited,
                        "Composer finished but didn't create the project.",
                    ));
                }
            }
        }
        check()?;

        // WordPress database + wp-config.php (written before anything leaves staging).
        let mut created_db: Option<(String, String)> = None;
        if let Some((service_id, name, port)) = &r.database {
            journal.progress(op_id, &format!("Creating the database {name}"), None, None)?;
            let sample =
                std::fs::read_to_string(app_dir.join("wp-config-sample.php")).unwrap_or_default();
            self.databases
                .create_database(service_id, name)
                .map_err(|f| IpcError::new(f.code, f.message, true))?;
            created_db = Some((service_id.clone(), name.clone()));
            match wp_config(&sample, name, *port, random_phrase) {
                Ok(Some(text)) => std::fs::write(app_dir.join("wp-config.php"), text)
                    .map_err(|_| failed(ErrorCode::Internal, "Couldn't write wp-config.php."))
                    .inspect_err(|_| self.undo_db(&created_db))?,
                Ok(None) => {
                    let _ = journal.event(
                        op_id,
                        EventLevel::Warn,
                        "wp-config-sample.php changed shape; WordPress's setup will ask for the database details.",
                        &serde_json::json!({}),
                    );
                }
                Err(e) => {
                    self.undo_db(&created_db);
                    return Err(e);
                }
            }
        }

        // Into www (one rename on the same volume), then register.
        journal.progress(op_id, "Adding the project", None, None)?;
        let target = &r.target;
        let target_existed = target.is_dir();
        if target_existed {
            std::fs::remove_dir(target)
                .map_err(|_| invalid(format!("{} isn't empty any more.", target.display())))
                .inspect_err(|_| self.undo_db(&created_db))?;
        } else if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::rename(&app_dir, target) {
            if target_existed {
                let _ = std::fs::create_dir(target);
            }
            self.undo_db(&created_db);
            return Err(failed(
                ErrorCode::Internal,
                format!("Couldn't move the project into {}: {e}", target.display()),
            ));
        }
        let mut service_ids = r.req.service_ids.clone();
        if let Some((id, _)) = &created_db
            && !service_ids.contains(id)
        {
            service_ids.push(id.clone());
        }
        let registered = match &r.existing {
            // An existing project keeps its services; the new database is added to them.
            Some(p) => self
                .db
                .write(|c| {
                    let mut ids = crate::persistence::projects::service_ids(c, &p.id)?;
                    for id in &service_ids {
                        if !ids.contains(id) {
                            ids.push(id.clone());
                        }
                    }
                    crate::persistence::projects::set_service_ids(c, &p.id, &ids)?;
                    crate::persistence::projects::get(c, &p.id)
                })
                .map_err(IpcError::from),
            None => projects::create_as(
                &self.db,
                &self.paths,
                project_id,
                &r.req.name,
                &r.req.folder,
                &service_ids,
            ),
        };
        let project = match registered {
            Ok(p) => p,
            Err(e) => {
                // Put the folder back into staging (removed with it) and the empty one back.
                let _ = std::fs::rename(target, &app_dir);
                if target_existed {
                    let _ = std::fs::create_dir(target);
                }
                self.undo_db(&created_db);
                return Err(e);
            }
        };

        if let Some(site) = &r.req.site {
            journal.progress(op_id, "Creating the site", None, None)?;
            match projects::create_site(&self.db, &self.sites, &project, site) {
                Ok(site_op) => {
                    let _ = journal.event(
                        op_id,
                        EventLevel::Info,
                        "Site creation started",
                        &serde_json::json!({ "siteOperationId": site_op }),
                    );
                }
                Err(e) => {
                    // The project is ready; the site can be added from its page.
                    let _ = journal.event(
                        op_id,
                        EventLevel::Warn,
                        &format!("The site wasn't created: {}", e.message),
                        &serde_json::json!({}),
                    );
                }
            }
        }
        Ok(())
    }

    fn undo_db(&self, created: &Option<(String, String)>) {
        if let Some((service_id, name)) = created {
            let _ = self.databases.drop_new_database(service_id, name);
        }
    }
}

/// Requirements per app, for the form before anything is chosen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub app: App,
    pub label: String,
    pub needs_composer: bool,
    pub min_php: String,
    pub php_extensions: Vec<String>,
    pub uses_database: bool,
}

pub fn catalog_of_apps() -> Vec<AppInfo> {
    App::ALL
        .iter()
        .map(|a| AppInfo {
            app: *a,
            label: a.label().into(),
            needs_composer: a.composer_package().is_some(),
            min_php: format!("{}.{}", a.min_php().0, a.min_php().1),
            php_extensions: a.php_extensions().iter().map(|s| (*s).to_owned()).collect(),
            uses_database: *a == App::Wordpress,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_names_come_from_the_folder_name() {
        assert_eq!(database_name("my-blog"), "my_blog");
        assert_eq!(database_name(r"clients\Acme Shop\"), "acme_shop");
        assert_eq!(database_name("clients/--x--"), "x");
        assert_eq!(database_name("é"), "wordpress");
        assert!(crate::services::databases::check_db_name(&database_name(&"a".repeat(90))).is_ok());
    }

    /// The real wp-config-sample.php from WordPress 7.1.4 (placeholders as shipped).
    const SAMPLE: &str = "define( 'DB_NAME', 'database_name_here' );\n\
define( 'DB_USER', 'username_here' );\n\
define( 'DB_PASSWORD', 'password_here' );\n\
define( 'DB_HOST', 'localhost' );\n\
define( 'AUTH_KEY',         'put your unique phrase here' );\n\
define( 'SECURE_AUTH_KEY',  'put your unique phrase here' );\n\
define( 'LOGGED_IN_KEY',    'put your unique phrase here' );\n\
define( 'NONCE_KEY',        'put your unique phrase here' );\n\
define( 'AUTH_SALT',        'put your unique phrase here' );\n\
define( 'SECURE_AUTH_SALT', 'put your unique phrase here' );\n\
define( 'LOGGED_IN_SALT',   'put your unique phrase here' );\n\
define( 'NONCE_SALT',       'put your unique phrase here' );\n\
$table_prefix = 'wp_';\n";

    #[test]
    fn wp_config_gets_the_database_and_fresh_keys() {
        let text = wp_config(SAMPLE, "my_blog", 3307, random_phrase)
            .unwrap()
            .unwrap();
        assert!(text.contains("define( 'DB_NAME', 'my_blog' );"));
        assert!(text.contains("define( 'DB_USER', 'root' );"));
        assert!(text.contains("define( 'DB_PASSWORD', '' );"));
        assert!(text.contains("define( 'DB_HOST', '127.0.0.1:3307' );"));
        assert!(!text.contains("put your unique phrase here"));
        let keys: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("_KEY'") || l.contains("_SALT'"))
            .map(|l| l.split('\'').nth(3).unwrap())
            .collect();
        assert_eq!(keys.len(), 8);
        assert!(
            keys.iter()
                .all(|k| k.len() == 64 && !k.contains(['\'', '\\']))
        );
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 8, "each key is different");
        // A sample without the placeholders isn't guessed at.
        assert_eq!(
            wp_config("<?php // nothing", "x", 3306, random_phrase).unwrap(),
            None
        );
    }

    #[test]
    fn app_requirements() {
        assert!(App::Laravel.php_extensions().contains(&"pdo_sqlite"));
        assert!(App::Laravel.php_extensions().contains(&"openssl"));
        assert!(App::Wordpress.php_extensions().contains(&"mysqli"));
        assert_eq!(App::Wordpress.composer_package(), None);
        assert_eq!(App::Symfony.composer_package(), Some("symfony/skeleton"));
        assert!(php_minor("8.1.34") < App::Laravel.min_php());
        assert!(php_minor("8.4.27-1") >= App::Laravel.min_php());
        let apps = catalog_of_apps();
        assert_eq!(apps.len(), 3);
        assert!(apps.iter().all(|a| a.needs_composer != a.uses_database));
    }

    /// Shaped like packagist.org's p2 files (Composer 2 "minified": `require` carries over).
    const LARAVEL_P2: &str = r#"{"minified":"composer/2.0","packages":{"laravel/laravel":[
        {"version":"v13.11.0","version_normalized":"13.11.0.0","require":{"php":"^8.3","laravel/framework":"^13.17"}},
        {"version":"v13.10.1","version_normalized":"13.10.1.0"},
        {"version":"v12.9.0","version_normalized":"12.9.0.0","require":{"php":"^8.2"}},
        {"version":"v12.0.0-beta1","version_normalized":"12.0.0.0-beta1"},
        {"version":"v11.6.1","version_normalized":"11.6.1.0"},
        {"version":"v10.3.3","version_normalized":"10.3.3.0","require":{"php":"^8.1"}}
    ]}}"#;
    const SYMFONY_P2: &str = r#"{"minified":"composer/2.0","packages":{"symfony/skeleton":[
        {"version":"v8.1.99","version_normalized":"8.1.99.0","require":{"php":">=8.4","ext-ctype":"*"}},
        {"version":"v8.0.99","version_normalized":"8.0.99.0"},
        {"version":"v7.4.99","version_normalized":"7.4.99.0","require":{"php":">=8.2"}},
        {"version":"v7.3.99","version_normalized":"7.3.99.0"},
        {"version":"v6.4.99","version_normalized":"6.4.99.0","require":{"php":">=8.1"}}
    ]}}"#;

    #[test]
    fn release_lines_come_from_packagist_with_their_php_needs() {
        let lines = release_lines(LARAVEL_P2, App::Laravel, (8, 2), 3);
        let summary: Vec<_> = lines
            .iter()
            .map(|l| {
                (
                    l.value.as_str(),
                    l.latest.as_str(),
                    l.min_php.as_deref(),
                    l.supported,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("13.*", "13.11.0", Some("8.3"), false),
                ("12.*", "12.9.0", Some("8.2"), true),
                // Inherits ^8.2 from the entry before it (minified); the beta is skipped.
                ("11.*", "11.6.1", Some("8.2"), true),
            ]
        );
        assert_eq!(lines[0].label, "Laravel 13.x");

        let lines = release_lines(SYMFONY_P2, App::Symfony, (8, 4), 10);
        let labels: Vec<_> = lines.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Symfony 8.1",
                "Symfony 8.0",
                "Symfony 7.4 LTS",
                "Symfony 7.3",
                "Symfony 6.4 LTS"
            ]
        );
        assert_eq!(lines[2].value, "7.4.*");
        assert!(lines.iter().all(|l| l.supported));
        assert!(release_lines("not json", App::Laravel, (8, 4), 3).is_empty());
        assert!(release_lines(LARAVEL_P2, App::Wordpress, (8, 4), 3).is_empty());
    }

    #[test]
    fn php_constraints_and_release_lines_are_checked() {
        assert_eq!(min_php_of("^8.3"), Some((8, 3)));
        assert_eq!(min_php_of(">=8.4"), Some((8, 4)));
        assert_eq!(min_php_of("^8.3|^8.2"), Some((8, 2)));
        assert_eq!(min_php_of("*"), None);
        for ok in ["13.*", "7.4.*"] {
            assert!(valid_line(ok), "{ok}");
        }
        // Passed to Composer as an argument: only plain release lines, never options.
        for bad in [
            "13",
            "^13.0",
            "--dev",
            "13.* --no-scripts",
            "dev-master",
            "",
        ] {
            assert!(!valid_line(bad), "{bad}");
        }
    }

    #[test]
    fn apps_install_only_on_the_drive_of_the_staging_folder() {
        assert!(same_volume(
            Path::new(r"D:\Devluma\www\a"),
            Path::new(r"d:\Devluma\staging")
        ));
        assert!(!same_volume(
            Path::new(r"E:\code\a"),
            Path::new(r"D:\Devluma\staging")
        ));
    }

    /// Real PHP, Nginx and MariaDB; Composer and WordPress downloaded by the app installer
    /// itself; Laravel and Symfony from packagist.org. Each project gets its site and is
    /// fetched over HTTP. `cargo test live_app_projects -- --ignored` (~250 MB plus packages).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network and real services"]
    async fn live_app_projects() {
        use crate::domain::operation::OperationStatus;
        use crate::persistence::operations as ops;
        use crate::services::download::DownloadOptions;
        use crate::services::install::{InstallContext, ProcessProbe};
        use crate::services::journal::test_support::RecordingSink;
        use crate::services::supervisor::tests::{LiveEnv, free_port};

        let env = LiveEnv::install(&[
            ("php", "8.4.27-1"),
            ("nginx", "1.30.5"),
            ("mariadb", "11.8.9"),
        ])
        .await;
        let php = env.installation("php");
        env.db
            .write(|c| crate::persistence::installations::set_global_default(c, &php.id))
            .unwrap();
        let web_port = free_port();
        let web = env.create("nginx", "web", web_port, None).await;
        env.start(&web).await;
        let maria = env.create("mariadb", "db", free_port(), None).await;
        env.start(&maria).await;

        let sink: Arc<dyn ProgressSink> = Arc::new(RecordingSink::default());
        let probe: Arc<dyn ExecutableProbe> = Arc::new(ProcessProbe);
        let catalog = Arc::new(crate::services::catalog::load());
        let (hosts, _hosts_file) = crate::services::hosts::test_support::service(
            Arc::clone(&env.db),
            &env.paths,
            "127.0.0.1 localhost\r\n",
        );
        let (certs, _store) =
            crate::services::certificates::test_support::service(Arc::clone(&env.db), &env.paths);
        let sites = SiteService::new(
            Arc::clone(&env.db),
            env.paths.clone(),
            Arc::clone(&env.sup),
            Arc::new(hosts),
            Arc::new(certs),
            Arc::clone(&sink),
        );
        let config = ConfigService::new(
            Arc::clone(&env.db),
            Arc::clone(&env.sup),
            Arc::clone(&probe),
            Arc::clone(&sink),
        );
        let databases = DatabaseService::new(
            Arc::clone(&env.db),
            env.paths.clone(),
            Arc::clone(&env.sup),
            Arc::clone(&sink),
        );
        let installs = Arc::new(InstallManager::default());
        let apps = AppsService::new(
            Arc::clone(&env.db),
            Arc::clone(&catalog),
            env.paths.clone(),
            Arc::clone(&probe),
            Arc::clone(&sink),
            config,
            Arc::clone(&databases),
            sites,
            installs,
            Arc::new(Tasks::default()),
        );
        let ctx = InstallContext {
            db: Arc::clone(&env.db),
            catalog,
            paths: env.paths.clone(),
            probe,
            sink,
            download: DownloadOptions::default(),
        };
        let db = Arc::clone(&env.db);
        let wait = |op: String| {
            let db = Arc::clone(&db);
            async move {
                for _ in 0..2400 {
                    let o = db.read(|c| ops::get(c, &op)).unwrap();
                    if !matches!(o.status, OperationStatus::Queued | OperationStatus::Running) {
                        return o;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                panic!("operation {op} didn't finish");
            }
        };
        let sites_settled = || {
            let db = Arc::clone(&db);
            async move {
                for _ in 0..240 {
                    let busy = db
                        .read(|c| ops::list_for_display(c, 50))
                        .unwrap()
                        .into_iter()
                        .any(|o| {
                            o.operation_type == "apply_site"
                                && matches!(
                                    o.status,
                                    OperationStatus::Queued | OperationStatus::Running
                                )
                        });
                    if !busy {
                        let failed: Vec<_> = db
                            .read(|c| ops::list_for_display(c, 50))
                            .unwrap()
                            .into_iter()
                            .filter(|o| {
                                o.operation_type == "apply_site"
                                    && o.status != OperationStatus::Succeeded
                            })
                            .map(|o| o.safe_error_message)
                            .collect();
                        assert!(failed.is_empty(), "site failed: {failed:?}");
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                panic!("site didn't finish");
            }
        };
        let get = |domain: &'static str| async move {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let r = client
                .get(format!("http://127.0.0.1:{web_port}/"))
                .header("Host", domain)
                .send()
                .await
                .unwrap();
            let status = r.status().as_u16();
            let location = r
                .headers()
                .get("location")
                .map(|v| v.to_str().unwrap_or_default().to_owned());
            (status, location, r.text().await.unwrap_or_default())
        };
        let site = |domain: &str| {
            Some(SiteOptions {
                domain: domain.into(),
                web_server_service_id: web.clone(),
                https: false,
            })
        };

        // 1. Laravel: Composer is downloaded, extensions turned on, create-project runs its
        //    scripts (key, SQLite database, migrations).
        let req = AppProjectRequest {
            name: "Api".into(),
            folder: "api".into(),
            app: App::Laravel,
            service_ids: vec![],
            site: site("api.test"),
            database_service_id: None,
            version: Some("12.*".into()),
            project_id: None,
        };
        let plan = apps.preview(&req);
        assert_eq!(plan.blocked, None);
        assert!(
            plan.downloads
                .iter()
                .any(|d| d.starts_with("Composer 2.11.2")),
            "{plan:?}"
        );
        assert!(
            plan.enable_extensions.contains(&"openssl".to_owned()),
            "{plan:?}"
        );
        let started = apps.start(ctx.clone(), &req).unwrap();
        let op = wait(started.operation_id.clone()).await;
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}\n{}",
            op.safe_error_message,
            composer::log_tail(&composer::log_path(&env.paths, &started.operation_id), 40)
        );
        let api = env.paths.www.join("api");
        assert!(api.join("artisan").is_file() && api.join(r"vendor\autoload.php").is_file());
        // The chosen release line, not the newest.
        let lock: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(api.join("composer.lock")).unwrap())
                .unwrap();
        let framework = lock["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "laravel/framework")
            .unwrap()["version"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            framework.starts_with("v12."),
            "Laravel 12.x, got {framework}"
        );
        let dotenv = std::fs::read_to_string(api.join(".env")).unwrap();
        assert!(
            dotenv.lines().any(|l| l.starts_with("APP_KEY=base64:")),
            "key generated"
        );
        assert!(
            api.join(r"database\database.sqlite").is_file(),
            "SQLite database created"
        );
        let project = db
            .read(|c| crate::persistence::projects::get(c, &started.project_id))
            .unwrap();
        assert_eq!(project.project_type.as_deref(), Some("laravel"));
        assert!(
            composer::missing_extensions(
                &ProcessProbe,
                Path::new(&php.install_path),
                &App::Laravel.php_extensions()
            )
            .unwrap()
            .is_empty()
        );
        sites_settled().await;
        let (status, _, body) = get("api.test").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("Laravel"), "{}", &body[..body.len().min(400)]);
        // A second app into the same, now non-empty folder is refused before anything runs.
        assert!(apps.preview(&req).blocked.unwrap().contains("isn't empty"));

        // 2. Symfony, with the Composer that's now installed.
        let req = AppProjectRequest {
            name: "Shop".into(),
            folder: "shop".into(),
            app: App::Symfony,
            service_ids: vec![],
            site: site("shop.test"),
            database_service_id: None,
            version: None,
            project_id: None,
        };
        assert!(apps.preview(&req).downloads.is_empty());
        let started = apps.start(ctx.clone(), &req).unwrap();
        let op = wait(started.operation_id.clone()).await;
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}\n{}",
            op.safe_error_message,
            composer::log_tail(&composer::log_path(&env.paths, &started.operation_id), 40)
        );
        assert!(env.paths.www.join(r"shop\bin\console").is_file());
        sites_settled().await;
        let (_, _, body) = get("shop.test").await;
        assert!(body.contains("Symfony"), "{}", &body[..body.len().min(400)]);

        // 3. WordPress: downloaded, copied, database created, wp-config.php written; an
        //    uninstalled WordPress that can reach its database redirects to its installer.
        let req = AppProjectRequest {
            name: "News".into(),
            folder: "news".into(),
            app: App::Wordpress,
            service_ids: vec![],
            site: site("news.test"),
            database_service_id: Some(maria.clone()),
            version: Some("7.0.8".into()),
            project_id: None,
        };
        let plan = apps.preview(&req);
        assert_eq!(plan.database_name.as_deref(), Some("news"));
        assert!(
            plan.downloads
                .iter()
                .any(|d| d.starts_with("WordPress 7.0.8")),
            "{plan:?}"
        );
        let started = apps.start(ctx.clone(), &req).unwrap();
        let op = wait(started.operation_id.clone()).await;
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
        // Downloaded into the project only: nothing installed in bin.
        assert!(!env.paths.packages.join("wordpress").exists());
        assert!(
            std::fs::read_to_string(env.paths.www.join(r"news\wp-includes\version.php"))
                .unwrap()
                .contains("$wp_version = '7.0.8';")
        );
        let wp_config = std::fs::read_to_string(env.paths.www.join(r"news\wp-config.php")).unwrap();
        assert!(wp_config.contains("define( 'DB_NAME', 'news' );"));
        assert!(!wp_config.contains("put your unique phrase here"));
        assert!(
            databases
                .databases(&maria)
                .unwrap()
                .contains(&"news".to_owned())
        );
        let services = db
            .read(|c| crate::persistence::projects::service_ids(c, &started.project_id))
            .unwrap();
        assert!(services.contains(&maria), "the database service is linked");
        sites_settled().await;
        let (status, location, body) = get("news.test").await;
        assert_eq!(status, 302, "{}", &body[..body.len().min(400)]);
        assert!(
            location
                .unwrap_or_default()
                .contains("wp-admin/install.php")
        );
        // The same folder name again: the database exists, so it's refused up front.
        std::fs::remove_dir_all(env.paths.www.join("news")).unwrap();
        assert!(
            apps.preview(&req)
                .blocked
                .unwrap()
                .contains("already has a database named news")
        );
    }
}
