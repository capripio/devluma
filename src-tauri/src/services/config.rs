//! Simple settings and the advanced config editor, for PHP versions and services (ADR 0018).
//!
//! Every change goes the same way, whether it came from the Settings form or the editor:
//! preview (exact diff) → back up the old file → write atomically → the tool's own check →
//! roll back on failure → reload a running web server. Nothing is written that the tool
//! itself hasn't accepted.
//!
//! - **PHP**: settings and extensions are edited in php.ini; the candidate is checked with
//!   `php -c <candidate> -m` before it replaces the file. Only problems the current php.ini
//!   doesn't already have block a save.
//! - **Services**: files the tool owns (my.ini, postgresql.conf, nginx/Apache `custom.conf`)
//!   are swapped in and checked in place (`ServiceBehavior::check_config`) while the service
//!   is claimed, then restored if the check fails. Settings for configs Devluma generates are
//!   kept in `service_settings` and rendered before every start.
//! - Text shown in the editor is never redacted-and-saved: a file that redaction would change
//!   (it holds a password or token) is shown read-only.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::install::ExecutableProbe;
use super::journal::{Journal, ProgressSink};
use super::php;
use super::supervisor::Supervisor;
use super::supervisor::behavior::{ConfigFile, Failure, ServiceBehavior, ServiceCtx, failure};
use crate::domain::adapter::AdapterId;
use crate::domain::config_file::{self, Dialect};
use crate::domain::config_settings::{self, Applies, Schema, SettingSpec, Store};
use crate::domain::service::ServiceState;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::NewOperation;
use crate::persistence::{Database, service_settings, services};
use crate::security::{paths as pathpolicy, redact};
use crate::state::DataPaths;

/// Larger files open read-only (they aren't hand-edited configs).
const MAX_EDITABLE: u64 = 512 * 1024;
/// Backups kept per file; older ones are pruned.
const KEEP_BACKUPS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConfigTarget {
    Php { installation_id: String },
    Service { service_id: String },
}

impl ConfigTarget {
    /// Folder name for backups; ids are ULIDs, so this is a plain name.
    fn scope(&self) -> String {
        match self {
            Self::Php { installation_id } => format!("php-{installation_id}"),
            Self::Service { service_id } => format!("service-{service_id}"),
        }
    }

    fn journal_target(&self) -> (&'static str, String) {
        match self {
            Self::Php { installation_id } => ("package_installation", installation_id.clone()),
            Self::Service { service_id } => ("service_instance", format!("service:{service_id}")),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConfigChange {
    /// Changed settings only. `None` = back to the tool's default.
    Settings {
        #[serde(default)]
        values: BTreeMap<String, Option<String>>,
        /// PHP only: extension name → enabled.
        #[serde(default)]
        extensions: BTreeMap<String, bool>,
    },
    /// The whole text of one editable file, from the editor.
    File {
        file: String,
        text: String,
        /// Hash of the file when it was opened, so an edit made elsewhere isn't overwritten.
        base_hash: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingView {
    #[serde(flatten)]
    pub spec: SettingSpec,
    /// Canonical value; `None` when not set (the default applies) or not readable.
    pub value: Option<String>,
    /// The file's own text, when Devluma can't show it as a simple value.
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionView {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupView {
    pub id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub key: String,
    pub label: String,
    pub path: String,
    pub editable: bool,
    /// Why an editable kind of file can't be edited here right now.
    pub read_only_reason: Option<String>,
    pub language: String,
    pub note: String,
    /// `None` when the file doesn't exist yet.
    pub text: Option<String>,
    /// SHA-256 of the bytes on disk (`""` when missing); send it back with a file change.
    pub hash: String,
    pub backups: Vec<BackupView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub settings: Vec<SettingView>,
    /// File the settings are written to; `None` when Devluma keeps them and generates the
    /// config.
    pub settings_file: Option<String>,
    /// Why the settings can't be changed right now (e.g. no php.ini yet).
    pub settings_unavailable: Option<String>,
    pub applies: Option<Applies>,
    pub extensions: Option<Vec<ExtensionView>>,
    pub files: Vec<FileView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPreview {
    pub path: String,
    /// Unified diff, redacted. Empty when nothing would change.
    pub diff: String,
    /// What checks the result before it's kept.
    pub check: String,
    pub applies: Applies,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartHint {
    pub service_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigApplied {
    pub message: String,
    /// Non-blocking remarks from the check (e.g. a value the tool adjusted).
    pub warnings: Vec<String>,
    /// Running services that keep the old configuration until restarted.
    pub restart: Vec<RestartHint>,
    pub view: ConfigView,
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, message, false)
}

fn ipc(f: Failure) -> IpcError {
    IpcError::new(f.code, f.message, true)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Line endings follow the file on disk (the editor always sends `\n`).
fn match_line_endings(old: Option<&str>, new: &str) -> String {
    let new = new.replace("\r\n", "\n");
    if old.is_some_and(|o| o.contains("\r\n")) {
        new.replace('\n', "\r\n")
    } else {
        new
    }
}

/// What a change resolves to.
enum Plan {
    /// Replace one file's text.
    File {
        file: ConfigFileRef,
        old: Option<String>,
        new: String,
    },
    /// Store generated-config settings (service only).
    Managed {
        old: BTreeMap<String, String>,
        new: BTreeMap<String, String>,
        old_render: String,
        new_render: String,
        label: String,
    },
}

#[derive(Clone)]
struct ConfigFileRef {
    key: String,
    path: PathBuf,
}

/// Resolved target: everything needed to read, check and write it. Lives for one call.
#[allow(clippy::large_enum_variant)]
enum Resolved {
    Php {
        installation_id: String,
        install: PathBuf,
    },
    Service {
        svc: crate::persistence::services::ServiceInstance,
        ctx: ServiceCtx,
        beh: &'static dyn ServiceBehavior,
        adapter: AdapterId,
    },
}

impl Resolved {
    fn schema(&self) -> Option<Schema> {
        match self {
            Self::Php { .. } => config_settings::schema(AdapterId::Php),
            Self::Service { adapter, .. } => config_settings::schema(*adapter),
        }
    }

    fn files(&self) -> Vec<ConfigFile> {
        match self {
            Self::Php { install, .. } => vec![ConfigFile {
                key: "main",
                label: "php.ini",
                path: install.join(php::INI_FILE),
                editable: true,
                language: "ini",
                note: "Shared by this PHP version's command line, terminals, scheduled tasks and PHP backends. PHP checks the file when you save.",
            }],
            Self::Service { ctx, beh, .. } => beh.config_files(ctx),
        }
    }

    fn file(&self, key: &str) -> Option<ConfigFile> {
        self.files().into_iter().find(|f| f.key == key)
    }

    /// When a saved change takes effect. Web servers reload for any of their files.
    fn applies(&self) -> Applies {
        match self {
            Self::Service {
                adapter: AdapterId::Nginx | AdapterId::Apache | AdapterId::Caddy,
                ..
            } => Applies::Reload,
            _ => self.schema().map_or(Applies::Restart, |s| s.applies),
        }
    }
}

pub struct ConfigService {
    db: Arc<Database>,
    supervisor: Arc<Supervisor>,
    probe: Arc<dyn ExecutableProbe>,
    progress: Arc<dyn ProgressSink>,
    paths: DataPaths,
    /// PHP versions with a change in progress (services use the supervisor's claim).
    busy_php: Mutex<HashSet<String>>,
}

struct PhpGuard<'a>(&'a ConfigService, String);

impl Drop for PhpGuard<'_> {
    fn drop(&mut self) {
        self.0
            .busy_php
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.1);
    }
}

impl ConfigService {
    pub fn new(
        db: Arc<Database>,
        supervisor: Arc<Supervisor>,
        probe: Arc<dyn ExecutableProbe>,
        progress: Arc<dyn ProgressSink>,
    ) -> Arc<Self> {
        let paths = supervisor.paths().clone();
        Arc::new(Self {
            db,
            supervisor,
            probe,
            progress,
            paths,
            busy_php: Mutex::new(HashSet::new()),
        })
    }

    fn resolve(&self, target: &ConfigTarget) -> Result<Resolved, IpcError> {
        match target {
            ConfigTarget::Php { installation_id } => {
                let inst = self.db.read(|c| installations::get(c, installation_id))?;
                if inst.package_id != "php" || inst.status != InstallStatus::Installed {
                    return Err(invalid("That isn't an installed PHP version."));
                }
                let install = pathpolicy::ensure_within(
                    &self.paths.packages,
                    Path::new(&inst.install_path),
                )
                .map_err(|_| {
                    invalid("This PHP installation's folder is missing or outside the packages directory.")
                })?;
                Ok(Resolved::Php {
                    installation_id: installation_id.clone(),
                    install,
                })
            }
            ConfigTarget::Service { service_id } => {
                let (svc, ctx, beh) = self.supervisor.config_context(service_id).map_err(ipc)?;
                let adapter = AdapterId::parse(&svc.lifecycle_adapter)
                    .ok_or_else(|| invalid("This service type isn't supported by this build."))?;
                Ok(Resolved::Service {
                    svc,
                    ctx,
                    beh,
                    adapter,
                })
            }
        }
    }

    // ---- view --------------------------------------------------------------------------

    pub fn view(&self, target: &ConfigTarget) -> Result<ConfigView, IpcError> {
        let resolved = self.resolve(target)?;
        if let Resolved::Service { ctx, beh, .. } = &resolved {
            // Creates custom.conf and the generated config if the service never started.
            let _ = beh.prepare(ctx);
        }
        let files: Vec<FileView> = resolved
            .files()
            .into_iter()
            .map(|f| self.file_view(target, &f))
            .collect();
        let schema = resolved.schema();
        let mut view = ConfigView {
            settings: Vec::new(),
            settings_file: None,
            settings_unavailable: None,
            applies: schema.map(|_| resolved.applies()),
            extensions: None,
            files,
        };
        let Some(schema) = schema else {
            return Ok(view);
        };
        match schema.store {
            Store::Managed => {
                let Resolved::Service { ctx, .. } = &resolved else {
                    return Ok(view);
                };
                view.settings = schema
                    .specs
                    .iter()
                    .map(|spec| {
                        let stored = ctx.settings.get(spec.key);
                        let value = stored.and_then(|v| config_settings::validate(spec, v).ok());
                        SettingView {
                            spec: *spec,
                            raw: stored.filter(|_| value.is_none()).cloned(),
                            value,
                        }
                    })
                    .collect();
            }
            Store::File { dialect, .. } => {
                let file = resolved
                    .files()
                    .into_iter()
                    .find(|f| f.editable)
                    .ok_or_else(IpcError::internal)?;
                view.settings_file = Some(file.key.to_owned());
                let text = std::fs::read(&file.path).ok();
                let text = text.as_deref().map(String::from_utf8_lossy);
                match text.as_deref() {
                    None => {
                        view.settings_unavailable = Some(match resolved {
                            Resolved::Php { .. } => {
                                "This PHP version has no php.ini yet. Create one from a template first.".into()
                            }
                            _ => format!("{} doesn't exist yet.", file.path.display()),
                        });
                    }
                    Some(text) => {
                        if let Some(reason) = Self::unsuitable(&file.path) {
                            view.settings_unavailable = Some(reason);
                        }
                        view.settings = read_settings(text, dialect, &schema);
                        if matches!(resolved, Resolved::Php { .. }) {
                            view.extensions = Some(self.php_extensions(&resolved, text));
                        }
                    }
                }
            }
        }
        Ok(view)
    }

    fn php_extensions(&self, resolved: &Resolved, ini: &str) -> Vec<ExtensionView> {
        let Resolved::Php { install, .. } = resolved else {
            return Vec::new();
        };
        let enabled: HashSet<String> = config_file::php_extensions(ini).into_iter().collect();
        let mut names: Vec<String> = std::fs::read_dir(install.join("ext"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_lowercase();
                        name.strip_prefix("php_")
                            .and_then(|n| n.strip_suffix(".dll"))
                            .map(str::to_owned)
                    })
                    // Test-only modules PHP ships but that nobody should load.
                    .filter(|n| !matches!(n.as_str(), "dl_test" | "zend_test"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
            .into_iter()
            .map(|name| ExtensionView {
                enabled: enabled.contains(&name),
                name,
            })
            .collect()
    }

    /// Why a file can't be edited here (size, encoding), if it can't.
    fn unsuitable(path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > MAX_EDITABLE {
            return Some("This file is too large to edit here. Open it in your own editor.".into());
        }
        let bytes = std::fs::read(path).ok()?;
        if std::str::from_utf8(&bytes).is_err() {
            return Some(
                "This file isn't UTF-8 text, so Devluma won't edit it. Open it in your own editor."
                    .into(),
            );
        }
        None
    }

    fn file_view(&self, target: &ConfigTarget, f: &ConfigFile) -> FileView {
        let bytes = std::fs::read(&f.path).ok();
        let hash = bytes.as_deref().map(sha256).unwrap_or_default();
        let mut read_only_reason = None;
        let text = bytes.as_deref().map(|b| {
            let full = String::from_utf8_lossy(b);
            let shown: String = full.chars().take(MAX_EDITABLE as usize).collect();
            if holds_secret(&shown) {
                read_only_reason = Some(
                    "This file contains a password or token, which Devluma never shows. Edit it in your own editor."
                        .to_owned(),
                );
                redact::redact(&shown).into_owned()
            } else {
                shown
            }
        });
        if f.editable && read_only_reason.is_none() {
            read_only_reason = Self::unsuitable(&f.path);
        }
        FileView {
            key: f.key.into(),
            label: f.label.into(),
            path: f.path.display().to_string(),
            editable: f.editable,
            read_only_reason,
            language: f.language.into(),
            note: f.note.into(),
            text,
            hash,
            backups: self.list_backups(target, f.key),
        }
    }

    // ---- planning ------------------------------------------------------------------------

    fn plan(&self, resolved: &Resolved, change: &ConfigChange) -> Result<Plan, IpcError> {
        match change {
            ConfigChange::File {
                file,
                text,
                base_hash,
            } => {
                let f = resolved
                    .file(file)
                    .ok_or_else(|| invalid("That file isn't part of this configuration."))?;
                let view_reason = if f.editable {
                    let current = std::fs::read(&f.path).ok();
                    let shown = current
                        .as_deref()
                        .map(String::from_utf8_lossy)
                        .unwrap_or_default();
                    holds_secret(&shown)
                        .then(|| {
                            "This file contains a password or token; edit it in your own editor."
                                .to_owned()
                        })
                        .or_else(|| Self::unsuitable(&f.path))
                } else {
                    Some(format!(
                        "{} is generated by Devluma and can't be edited here.",
                        f.label
                    ))
                };
                if let Some(reason) = view_reason {
                    return Err(invalid(reason));
                }
                let current = std::fs::read(&f.path).ok();
                let current_hash = current.as_deref().map(sha256).unwrap_or_default();
                if &current_hash != base_hash {
                    return Err(invalid(format!(
                        "{} changed on disk since you opened it. Reload it to see the current version, then make your change again.",
                        f.label
                    )));
                }
                let old = current.map(|b| String::from_utf8_lossy(&b).into_owned());
                if text.len() as u64 > MAX_EDITABLE {
                    return Err(invalid("The text is too large to save."));
                }
                if text.contains('\0') {
                    return Err(invalid("The text contains a NUL character."));
                }
                let new = match_line_endings(old.as_deref(), text);
                Ok(Plan::File {
                    file: ConfigFileRef {
                        key: f.key.into(),
                        path: f.path,
                    },
                    old,
                    new,
                })
            }
            ConfigChange::Settings { values, extensions } => {
                let schema = resolved
                    .schema()
                    .ok_or_else(|| invalid("This service has no simple settings."))?;
                let mut checked: BTreeMap<String, Option<String>> = BTreeMap::new();
                for (key, value) in values {
                    let spec = schema
                        .spec(key)
                        .ok_or_else(|| invalid(format!("Unknown setting: {key}.")))?;
                    let value = value
                        .as_deref()
                        .map(|v| config_settings::validate(spec, v))
                        .transpose()
                        .map_err(invalid)?;
                    checked.insert(key.clone(), value);
                }
                match schema.store {
                    Store::Managed => {
                        if !extensions.is_empty() {
                            return Err(invalid("Extensions apply to PHP only."));
                        }
                        let Resolved::Service { ctx, beh, svc, .. } = resolved else {
                            return Err(IpcError::internal());
                        };
                        let old = ctx.settings.clone();
                        let mut new = old.clone();
                        for (k, v) in &checked {
                            match v {
                                Some(v) => new.insert(k.clone(), v.clone()),
                                None => new.remove(k),
                            };
                        }
                        let render = |settings: &BTreeMap<String, String>| {
                            let c = ServiceCtx {
                                settings: settings.clone(),
                                ..clone_ctx(ctx)
                            };
                            beh.render_config(&c).unwrap_or_default()
                        };
                        let label = beh
                            .config_files(ctx)
                            .into_iter()
                            .find(|f| !f.editable)
                            .map_or_else(
                                || svc.instance_name.clone(),
                                |f| f.path.display().to_string(),
                            );
                        Ok(Plan::Managed {
                            old_render: render(&old),
                            new_render: render(&new),
                            old,
                            new,
                            label,
                        })
                    }
                    Store::File { dialect, file } => {
                        let f = resolved
                            .files()
                            .into_iter()
                            .find(|f| f.editable)
                            .ok_or_else(IpcError::internal)?;
                        if let Some(reason) = Self::unsuitable(&f.path) {
                            return Err(invalid(reason));
                        }
                        let old = std::fs::read_to_string(&f.path).map_err(|_| {
                            invalid(match resolved {
                                Resolved::Php { .. } => "This PHP version has no php.ini yet. Create one from a template first.".to_owned(),
                                _ => format!("Could not read {file}."),
                            })
                        })?;
                        let mut new = old.clone();
                        for (key, value) in &checked {
                            let spec = schema.spec(key).ok_or_else(IpcError::internal)?;
                            let encoded = value
                                .as_deref()
                                .map(|v| config_settings::encode(schema.syntax, spec, v));
                            new = config_file::set(&new, dialect, key, encoded.as_deref());
                        }
                        if !extensions.is_empty() {
                            if !matches!(resolved, Resolved::Php { .. }) {
                                return Err(invalid("Extensions apply to PHP only."));
                            }
                            let available: HashSet<String> = self
                                .php_extensions(resolved, &old)
                                .into_iter()
                                .map(|e| e.name)
                                .collect();
                            for (name, enabled) in extensions {
                                if !available.contains(name) {
                                    return Err(invalid(format!(
                                        "This PHP version doesn't include the {name} extension."
                                    )));
                                }
                                new = config_file::set_php_extension(&new, name, *enabled);
                            }
                        }
                        Ok(Plan::File {
                            file: ConfigFileRef {
                                key: f.key.into(),
                                path: f.path,
                            },
                            old: Some(old),
                            new,
                        })
                    }
                }
            }
        }
    }

    fn check_label(resolved: &Resolved) -> String {
        match resolved {
            Resolved::Php { .. } => {
                "PHP starts with the new php.ini (php -m) and must report no new problems.".into()
            }
            Resolved::Service { adapter, .. } => match adapter {
                AdapterId::Nginx => "nginx -t checks the whole configuration.".into(),
                AdapterId::Apache => "httpd -t checks the whole configuration.".into(),
                AdapterId::Caddy => "caddy validate checks the whole configuration.".into(),
                AdapterId::Frankenphp => {
                    "frankenphp validate checks the whole configuration; PHP reports php.ini problems."
                        .into()
                }
                AdapterId::Mariadb => {
                    "mariadbd reads my.ini as it would at start and must accept every option."
                        .into()
                }
                AdapterId::Postgresql => {
                    "postgres reads postgresql.conf and must accept every parameter.".into()
                }
                _ => "Devluma checks every value against its allowed range.".into(),
            },
        }
    }

    pub fn preview(
        &self,
        target: &ConfigTarget,
        change: &ConfigChange,
    ) -> Result<ConfigPreview, IpcError> {
        let resolved = self.resolve(target)?;
        let plan = self.plan(&resolved, change)?;
        let (path, diff) = match &plan {
            Plan::File { file, old, new } => {
                let p = file.path.display().to_string();
                let d = crate::domain::hosts::diff(old.as_deref().unwrap_or(""), new, &p);
                (p, d)
            }
            Plan::Managed {
                old_render,
                new_render,
                label,
                ..
            } => (
                label.clone(),
                crate::domain::hosts::diff(old_render, new_render, label),
            ),
        };
        Ok(ConfigPreview {
            path,
            diff: redact::redact(&diff).into_owned(),
            check: Self::check_label(&resolved),
            applies: resolved.applies(),
        })
    }

    // ---- apply ---------------------------------------------------------------------------

    pub fn apply(
        self: &Arc<Self>,
        target: &ConfigTarget,
        change: &ConfigChange,
    ) -> Result<ConfigApplied, IpcError> {
        // One change at a time per target, and never during a service start or stop.
        let _service_guard = match target {
            ConfigTarget::Service { service_id } => Some(self.supervisor.claim(service_id)?),
            ConfigTarget::Php { .. } => None,
        };
        let _php_guard = match target {
            ConfigTarget::Php { installation_id } => {
                let mut busy = self.busy_php.lock().unwrap_or_else(|e| e.into_inner());
                if !busy.insert(installation_id.clone()) {
                    return Err(invalid(
                        "Another change to this PHP version's settings is in progress.",
                    ));
                }
                Some(PhpGuard(self, installation_id.clone()))
            }
            ConfigTarget::Service { .. } => None,
        };

        let resolved = self.resolve(target)?;
        let plan = self.plan(&resolved, change)?;
        let journal = Journal::new(&self.db, self.progress.as_ref());
        let (target_type, target_id) = target.journal_target();
        let op = journal.start(&NewOperation {
            operation_type: "edit_config",
            target_type: Some(target_type),
            target_id: Some(&target_id),
        })?;
        journal.running(&op.id)?;
        match self.execute(target, &resolved, plan) {
            Ok(mut applied) => {
                journal.succeed(&op.id)?;
                applied.view = self.view(target)?;
                Ok(applied)
            }
            Err(f) => {
                journal.fail(&op.id, f.code, &f.message)?;
                Err(IpcError::new(f.code, f.message, true))
            }
        }
    }

    fn execute(
        &self,
        target: &ConfigTarget,
        resolved: &Resolved,
        plan: Plan,
    ) -> Result<ConfigApplied, Failure> {
        let unchanged = match &plan {
            Plan::File { old, new, .. } => old.as_deref() == Some(new.as_str()),
            Plan::Managed { old, new, .. } => old == new,
        };
        if unchanged {
            return Ok(self.applied("Nothing changed.".into(), Vec::new(), Vec::new()));
        }
        match (resolved, plan) {
            (
                Resolved::Php {
                    install,
                    installation_id,
                },
                Plan::File { file, old, new },
            ) => self.apply_php(
                target,
                install,
                installation_id,
                &file,
                old.as_deref(),
                &new,
            ),
            (Resolved::Service { .. }, Plan::File { file, old, new }) => {
                self.backup(target, &file.key, old.as_deref())?;
                write_atomic(&file.path, &new)?;
                let restore = || match &old {
                    Some(text) => {
                        let _ = write_atomic(&file.path, text);
                    }
                    None => {
                        let _ = std::fs::remove_file(&file.path);
                    }
                };
                self.check_and_activate(resolved, restore)
            }
            (Resolved::Service { svc, .. }, Plan::Managed { old, new, .. }) => {
                let changes: BTreeMap<String, Option<String>> = old
                    .keys()
                    .chain(new.keys())
                    .map(|k| (k.clone(), new.get(k).cloned()))
                    .collect();
                self.db
                    .write(|c| service_settings::update(c, &svc.id, &changes))
                    .map_err(db_fail)?;
                let restore = || {
                    let _ = self
                        .db
                        .write(|c| service_settings::replace(c, &svc.id, &old));
                };
                // The settings are read into a fresh context.
                let fresh = self
                    .resolve(target)
                    .map_err(|e| failure(e.code, e.message))?;
                self.check_and_activate(&fresh, restore)
            }
            (Resolved::Php { .. }, Plan::Managed { .. }) => {
                Err(failure(ErrorCode::Internal, "Unexpected change for PHP."))
            }
        }
    }

    fn apply_php(
        &self,
        target: &ConfigTarget,
        install: &Path,
        installation_id: &str,
        file: &ConfigFileRef,
        old: Option<&str>,
        new: &str,
    ) -> Result<ConfigApplied, Failure> {
        let tmp = install.join(format!("{}.devluma-new", php::INI_FILE));
        write_atomic(&tmp, new)?;
        let check = |path: &Path| {
            php::ini_problems(self.probe.as_ref(), install, path).map_err(|e| {
                failure(
                    ErrorCode::ProcessExited,
                    format!("Could not run php.exe: {e}"),
                )
            })
        };
        let tmp_name = tmp.display().to_string();
        let ini_name = file.path.display().to_string();
        let candidate = check(&tmp).map(|problems| {
            // Messages name the file; show the real one.
            problems
                .into_iter()
                .map(|p| p.replace(&tmp_name, &ini_name))
                .collect::<Vec<_>>()
        });
        let current = if old.is_some() && file.path.is_file() {
            check(&file.path).unwrap_or_default()
        } else {
            Vec::new()
        };
        let candidate = match candidate {
            Ok(c) => c,
            Err(f) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(f);
            }
        };
        let new_problems: Vec<String> = candidate
            .iter()
            .filter(|p| !current.contains(p))
            .cloned()
            .collect();
        if !new_problems.is_empty() {
            let _ = std::fs::remove_file(&tmp);
            return Err(failure(
                ErrorCode::ConfigInvalid,
                format!(
                    "PHP reported a problem, so php.ini wasn't changed: {}",
                    new_problems.join(" ")
                ),
            ));
        }
        if let Err(f) = self.backup(target, &file.key, old) {
            let _ = std::fs::remove_file(&tmp);
            return Err(f);
        }
        std::fs::rename(&tmp, &file.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            failure(
                ErrorCode::Internal,
                format!("Could not replace php.ini: {e}"),
            )
        })?;

        let restart: Vec<RestartHint> = self
            .db
            .read(services::list)
            .map_err(db_fail)?
            .into_iter()
            .filter(|s| {
                s.package_installation_id == installation_id
                    && s.lifecycle_adapter == AdapterId::Php.as_str()
                    && s.observed_state.is_live()
            })
            .map(|s| RestartHint {
                service_id: s.id,
                name: s.instance_name,
            })
            .collect();
        let mut message = "Saved php.ini. PHP checked it and found no new problems. New terminals and scheduled tasks use it right away.".to_owned();
        if !restart.is_empty() {
            message.push_str(" Running PHP backends keep the old settings until they restart.");
        }
        Ok(self.applied(message, candidate, restart))
    }

    /// Regenerates and checks a service's configuration; on failure undoes the change with
    /// `restore` (and regenerates again). A running web server is reloaded.
    fn check_and_activate(
        &self,
        resolved: &Resolved,
        restore: impl Fn(),
    ) -> Result<ConfigApplied, Failure> {
        let Resolved::Service {
            svc,
            ctx,
            beh,
            adapter,
        } = resolved
        else {
            return Err(failure(ErrorCode::Internal, "Not a service."));
        };
        let undo = |f: Failure| -> Failure {
            restore();
            if let Ok((_, c, b)) = self.supervisor.config_context(&svc.id) {
                let _ = b.prepare(&c);
            }
            failure(
                f.code,
                format!("{} Nothing was changed.", f.message.trim_end()),
            )
        };
        let warnings = beh
            .prepare(ctx)
            .and_then(|()| beh.check_config(ctx))
            .map_err(undo)?;

        let running = matches!(
            svc.observed_state,
            ServiceState::Running | ServiceState::Degraded
        );
        let name = svc.instance_name.clone();
        if matches!(
            adapter,
            AdapterId::Nginx | AdapterId::Apache | AdapterId::Caddy
        ) {
            return match self.supervisor.reload_if_running(&svc.id) {
                Ok(true) => Ok(self.applied(
                    format!("Saved and checked. {name} reloaded and uses it now."),
                    warnings,
                    Vec::new(),
                )),
                Ok(false) => Ok(self.applied(
                    format!("Saved and checked. {name} uses it the next time it starts."),
                    warnings,
                    Vec::new(),
                )),
                Err(f) => {
                    let f = undo(f);
                    let _ = self.supervisor.reload_if_running(&svc.id);
                    Err(f)
                }
            };
        }
        if running {
            Ok(self.applied(
                format!("Saved and checked. Restart {name} to use the new configuration."),
                warnings,
                vec![RestartHint {
                    service_id: svc.id.clone(),
                    name,
                }],
            ))
        } else {
            Ok(self.applied(
                format!("Saved and checked. {name} uses it the next time it starts."),
                warnings,
                Vec::new(),
            ))
        }
    }

    fn applied(
        &self,
        message: String,
        warnings: Vec<String>,
        restart: Vec<RestartHint>,
    ) -> ConfigApplied {
        ConfigApplied {
            message,
            warnings: warnings
                .iter()
                .map(|w| redact::redact(w).into_owned())
                .collect(),
            restart,
            // Filled in by `apply` once the operation is recorded.
            view: ConfigView {
                settings: Vec::new(),
                settings_file: None,
                settings_unavailable: None,
                applies: None,
                extensions: None,
                files: Vec::new(),
            },
        }
    }

    // ---- backups -------------------------------------------------------------------------

    fn backup_dir(&self, target: &ConfigTarget, key: &str) -> PathBuf {
        self.paths
            .backups
            .join("configs")
            .join(target.scope())
            .join(key)
    }

    /// Copies the current text aside (if there is one) and prunes old copies.
    fn backup(&self, target: &ConfigTarget, key: &str, old: Option<&str>) -> Result<(), Failure> {
        let Some(old) = old else {
            return Ok(());
        };
        let dir = self.backup_dir(target, key);
        std::fs::create_dir_all(&dir).map_err(|e| {
            failure(
                ErrorCode::Internal,
                format!("Could not create the backup folder: {e}"),
            )
        })?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
        std::fs::write(dir.join(format!("{stamp}.bak")), old).map_err(|e| {
            failure(
                ErrorCode::Internal,
                format!("Could not back up the current file: {e}"),
            )
        })?;
        let mut names = backup_names(&dir);
        while names.len() > KEEP_BACKUPS {
            let oldest = names.remove(names.len() - 1);
            let _ = std::fs::remove_file(dir.join(oldest));
        }
        Ok(())
    }

    fn list_backups(&self, target: &ConfigTarget, key: &str) -> Vec<BackupView> {
        backup_names(&self.backup_dir(target, key))
            .into_iter()
            .filter_map(|name| {
                let stamp = name.strip_suffix(".bak")?;
                let at = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S%3fZ").ok()?;
                Some(BackupView {
                    created_at: at.and_utc().to_rfc3339(),
                    id: name,
                })
            })
            .collect()
    }

    /// A previous version's text, to load into the editor (it's saved through the normal
    /// flow, with its checks). Redacted text is refused, as in the editor.
    pub fn read_backup(
        &self,
        target: &ConfigTarget,
        file: &str,
        backup_id: &str,
    ) -> Result<String, IpcError> {
        let resolved = self.resolve(target)?;
        let f = resolved
            .file(file)
            .ok_or_else(|| invalid("That file isn't part of this configuration."))?;
        if !is_backup_name(backup_id) {
            return Err(invalid("Unknown backup."));
        }
        let path = self.backup_dir(target, f.key).join(backup_id);
        let text = std::fs::read_to_string(&path).map_err(|_| invalid("Unknown backup."))?;
        if holds_secret(&text) {
            return Err(invalid(
                "This backup contains a password or token, which Devluma never shows.",
            ));
        }
        Ok(text.replace("\r\n", "\n"))
    }
}

/// Keys whose names look like credentials but hold none (PostgreSQL's initdb writes them).
const NOT_SECRETS: &[&str] = &["password_encryption", "md5_password_warnings"];

/// Whether redaction would hide something on an active line. A file that does is shown
/// redacted and read-only: saving redacted text would overwrite the secret. Comment lines
/// (`#`, `;`) and `NOT_SECRETS` don't count; otherwise every postgresql.conf would be locked
/// by its `#password_encryption = scram-sha-256` line.
fn holds_secret(text: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim_start();
        if t.starts_with('#') || t.starts_with(';') {
            return false;
        }
        let key = t.split(['=', ' ', '\t']).next().unwrap_or("");
        if NOT_SECRETS.iter().any(|k| k.eq_ignore_ascii_case(key)) {
            return false;
        }
        redact::redact(line) != line
    })
}

fn clone_ctx(ctx: &ServiceCtx) -> ServiceCtx {
    ServiceCtx {
        service_id: ctx.service_id.clone(),
        install_dir: ctx.install_dir.clone(),
        service_dir: ctx.service_dir.clone(),
        data_dir: ctx.data_dir.clone(),
        port: ctx.port,
        admin_port: ctx.admin_port,
        settings: ctx.settings.clone(),
    }
}

fn db_fail(e: crate::persistence::DbError) -> Failure {
    let i = IpcError::from(e);
    failure(i.code, i.message)
}

/// `20261022T003320123Z.bak` — the only names `read_backup` accepts.
fn is_backup_name(name: &str) -> bool {
    name.strip_suffix(".bak").is_some_and(|stamp| {
        stamp.len() == 19
            && stamp.chars().enumerate().all(|(i, c)| match i {
                8 => c == 'T',
                18 => c == 'Z',
                _ => c.is_ascii_digit(),
            })
    })
}

/// Newest first.
fn backup_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| is_backup_name(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

/// Write to a temp file next to the target, then rename over it.
fn write_atomic(path: &Path, text: &str) -> Result<(), Failure> {
    let fail = |e: std::io::Error| {
        failure(
            ErrorCode::Internal,
            format!("Could not write {}: {e}", path.display()),
        )
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".devluma-tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(e)
    })
}

fn read_settings(text: &str, dialect: Dialect, schema: &Schema) -> Vec<SettingView> {
    schema
        .specs
        .iter()
        .map(|spec| {
            let raw = config_file::get(text, dialect, spec.key);
            let value = raw
                .as_deref()
                .and_then(|r| config_settings::decode(spec, r));
            SettingView {
                spec: *spec,
                raw: raw.filter(|_| value.is_none()),
                value,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
