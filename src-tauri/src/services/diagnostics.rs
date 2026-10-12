//! Diagnostic export (trd.md: diagnostics never contain secrets; design.md: show exactly
//! what's included). The export is built from the same sections the preview shows, so what
//! the user reviews is byte-for-byte what is written.
//!
//! Every section is passed through `security::redact` (known secrets, credentials in URLs,
//! auth headers, key=value secrets) and, by default, personal paths are masked
//! (`C:\Users\<name>` → `%USERPROFILE%`). Project files, database data, private keys, the
//! ngrok authtoken and the rest of the hosts file are never read into a section.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::supervisor::behavior::{Failure, failure};
use crate::error::ErrorCode;
use crate::persistence::{
    Database, DbError, backups, certificates, installations, operations, services, sites,
};
use crate::security::redact;
use crate::state::DataPaths;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    /// File name inside the export.
    pub file: String,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsPreview {
    pub sections: Vec<Section>,
    /// What is deliberately not included, in plain language.
    pub never_included: Vec<String>,
    pub paths_masked: bool,
}

pub struct AppFacts {
    pub version: String,
    pub debug_build: bool,
}

pub struct Diagnostics {
    db: Arc<Database>,
    paths: DataPaths,
    facts: AppFacts,
}

fn db_fail(e: DbError) -> Failure {
    let i = crate::error::IpcError::from(e);
    failure(i.code, i.message)
}

/// Replaces personal folders with environment-variable names (longest first).
pub fn mask_paths(text: &str) -> String {
    let mut vars: Vec<(String, String)> = ["LOCALAPPDATA", "APPDATA", "USERPROFILE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (format!("%{k}%"), v)))
        .filter(|(_, v)| v.len() > 3)
        .collect();
    vars.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut out = text.to_owned();
    for (name, value) in &vars {
        for variant in [value.clone(), value.replace('\\', "/")] {
            out = replace_ignore_case(&out, &variant, name);
        }
    }
    if let Ok(user) = std::env::var("USERNAME")
        && user.len() > 2
    {
        out = replace_ignore_case(&out, &format!("\\{user}\\"), "\\<user>\\");
        out = replace_ignore_case(&out, &format!("/{user}/"), "/<user>/");
    }
    out
}

fn replace_ignore_case(haystack: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return haystack.to_owned();
    }
    let lower_h = haystack.to_lowercase();
    let lower_n = needle.to_lowercase();
    // Only safe when lowercasing keeps byte offsets (ASCII-only paths); otherwise exact.
    if lower_h.len() != haystack.len() || lower_n.len() != needle.len() {
        return haystack.replace(needle, with);
    }
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while let Some(pos) = lower_h[i..].find(&lower_n) {
        out.push_str(&haystack[i..i + pos]);
        out.push_str(with);
        i += pos + needle.len();
    }
    out.push_str(&haystack[i..]);
    out
}

fn tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

impl Diagnostics {
    pub fn new(db: Arc<Database>, paths: DataPaths, facts: AppFacts) -> Arc<Self> {
        Arc::new(Self { db, paths, facts })
    }

    fn sections_raw(&self) -> Result<Vec<Section>, Failure> {
        let mut out = Vec::new();
        let section = |file: &str, title: &str, content: String| Section {
            file: file.to_owned(),
            title: title.to_owned(),
            content,
        };

        let schema: i64 = self
            .db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap_or(-1);
        out.push(section(
            "app.txt",
            "App and system",
            format!(
                "Devluma {} ({})\nWindows: {} {}\nData folder: {}\nDatabase schema version: {schema}\n",
                self.facts.version,
                if self.facts.debug_build { "debug build" } else { "release build" },
                std::env::consts::OS,
                std::env::consts::ARCH,
                self.paths.root.display(),
            ),
        ));

        let insts = self.db.read(installations::list).map_err(db_fail)?;
        let mut text = String::new();
        for i in &insts {
            text.push_str(&format!(
                "{} — {} — {}{}\n  {}\n",
                i.package_version_id,
                i.status.as_str(),
                i.integrity_result,
                if i.is_global_default {
                    " — default"
                } else {
                    ""
                },
                i.install_path
            ));
        }
        out.push(section("packages.txt", "Installed packages", text));

        let svcs = self.db.read(services::list).map_err(db_fail)?;
        let mut text = String::new();
        let mut logs = String::new();
        for s in &svcs {
            text.push_str(&format!(
                "{} ({}) — {} — port {}{} — health {} — last error {}\n",
                s.instance_name,
                s.lifecycle_adapter,
                s.observed_state.as_str(),
                s.port.map_or("—".into(), |p| p.to_string()),
                s.admin_port.map_or(String::new(), |p| format!(", UI {p}")),
                s.health_status,
                s.last_error_code.as_deref().unwrap_or("none"),
            ));
            let log = self
                .paths
                .services
                .join(&s.id)
                .join("logs")
                .join("service.log");
            if log.is_file() {
                logs.push_str(&format!(
                    "===== {} ({}) =====\n{}\n\n",
                    s.instance_name,
                    log.display(),
                    tail(&log, 100)
                ));
            }
        }
        out.push(section("services.txt", "Services", text));
        out.push(section(
            "service-logs.txt",
            "Service logs (last 100 lines each)",
            logs,
        ));

        let site_rows = self.db.read(sites::list).map_err(db_fail)?;
        let text: String = site_rows
            .iter()
            .map(|s| {
                format!(
                    "{} — port {}{} — status {} — hosts entry {}\n",
                    s.domain,
                    s.http_port,
                    if s.ssl_enabled { ", HTTPS" } else { "" },
                    s.status,
                    if s.hosts_managed {
                        "managed"
                    } else {
                        "not added"
                    }
                )
            })
            .collect();
        out.push(section("sites.txt", "Sites", text));

        let certs = self.db.read(certificates::list).map_err(db_fail)?;
        let text: String = certs
            .iter()
            .map(|c| {
                format!(
                    "{} ({}) — valid until {} — trusted {}\n",
                    c.common_name, c.issuer_type, c.not_after, c.trusted
                )
            })
            .collect();
        out.push(section("certificates.txt", "Certificates (no keys)", text));

        let bks = self.db.read(backups::list).map_err(db_fail)?;
        let text: String = bks
            .iter()
            .map(|b| {
                format!(
                    "{} {} {} {} bytes — {}\n",
                    b.created_at,
                    b.kind,
                    b.database_name.as_deref().unwrap_or("(data folder)"),
                    b.size_bytes,
                    b.path
                )
            })
            .collect();
        out.push(section("backups.txt", "Database backups (list only)", text));

        let ops = self
            .db
            .read(|c| operations::list_for_display(c, 100))
            .map_err(db_fail)?;
        let mut text = String::new();
        for o in &ops {
            text.push_str(&format!(
                "{} {} {} {}{}\n",
                o.started_at,
                o.operation_type,
                o.status.as_str(),
                o.target_id.as_deref().unwrap_or(""),
                o.safe_error_message
                    .as_deref()
                    .map_or(String::new(), |m| format!(" — {m}")),
            ));
            if o.status.as_str() == "failed" || o.status.as_str() == "needs_recovery" {
                for e in self
                    .db
                    .read(|c| operations::list_events(c, &o.id))
                    .map_err(db_fail)?
                {
                    text.push_str(&format!("    [{}] {}\n", e.level, e.message));
                }
            }
        }
        out.push(section(
            "operations.txt",
            "Recent operations (failed ones with their events)",
            text,
        ));
        Ok(out)
    }

    pub fn preview(&self, mask: bool) -> Result<DiagnosticsPreview, Failure> {
        let sections = self
            .sections_raw()?
            .into_iter()
            .map(|mut s| {
                s.content = redact::redact(&s.content).into_owned();
                if mask {
                    s.content = mask_paths(&s.content);
                }
                s
            })
            .collect();
        Ok(DiagnosticsPreview {
            sections,
            never_included: vec![
                "Your project files and database data".into(),
                "Private keys and the local CA key".into(),
                "The ngrok authtoken and other secrets".into(),
                "Your hosts file outside Devluma's own entries, and environment variables".into(),
            ],
            paths_masked: mask,
        })
    }

    /// Writes the previewed sections to `<root>/diagnostics/devluma-diagnostics-<time>.zip`.
    pub fn export(&self, mask: bool) -> Result<PathBuf, Failure> {
        let preview = self.preview(mask)?;
        let dir = self.paths.root.join("diagnostics");
        std::fs::create_dir_all(&dir).map_err(|e| failure(ErrorCode::Internal, e.to_string()))?;
        let file = dir.join(format!(
            "devluma-diagnostics-{}.zip",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
        ));
        let io = |e: std::io::Error| {
            failure(
                ErrorCode::Internal,
                format!("Could not write the export: {e}"),
            )
        };
        let f = std::fs::File::create(&file).map_err(io)?;
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let zerr = |e: zip::result::ZipError| {
            failure(
                ErrorCode::Internal,
                format!("Could not write the export: {e}"),
            )
        };
        for s in &preview.sections {
            zip.start_file(s.file.as_str(), opts).map_err(zerr)?;
            zip.write_all(s.content.as_bytes()).map_err(io)?;
        }
        zip.start_file("README.txt", opts).map_err(zerr)?;
        zip.write_all(
            format!(
                "Devluma diagnostic export. Secrets are redacted{}.\nNot included: {}.\n",
                if mask {
                    " and personal folders are masked"
                } else {
                    ""
                },
                preview.never_included.join("; ")
            )
            .as_bytes(),
        )
        .map_err(io)?;
        zip.finish().map_err(zerr)?;
        Ok(file)
    }

    pub fn export_dir(&self) -> PathBuf {
        self.paths.root.join("diagnostics")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::operations::NewOperation;
    use crate::persistence::test_support::db;
    use crate::services::journal::Journal;
    use crate::services::journal::test_support::RecordingSink;

    fn diag() -> (tempfile::TempDir, Arc<Diagnostics>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(&dir.path().join("Devluma"));
        paths.create_all().unwrap();
        let d = Diagnostics::new(
            Arc::new(db()),
            paths,
            AppFacts {
                version: "0.1.0".into(),
                debug_build: true,
            },
        );
        (dir, d)
    }

    #[test]
    fn secrets_never_reach_the_preview_or_the_export() {
        let (_dir, d) = diag();
        let token = "2xY9secretNgrokTokenValue_abcdef123456";
        redact::register_secret(token);
        let sink = RecordingSink::default();
        let journal = Journal::new(&d.db, &sink);
        let op = journal
            .start(&NewOperation {
                operation_type: "start_tunnel",
                target_type: None,
                target_id: None,
            })
            .unwrap();
        let _ = journal.fail(
            &op.id,
            ErrorCode::ProcessExited,
            &format!("ngrok said: bad token {token}; proxy http://bob:hunter2@proxy.local; password=hunter3"),
        );
        let preview = d.preview(true).unwrap();
        let all: String = preview.sections.iter().map(|s| s.content.clone()).collect();
        assert!(all.contains("start_tunnel"), "{all}");
        for secret in [token, "hunter2", "hunter3"] {
            assert!(!all.contains(secret), "{secret} leaked: {all}");
        }

        let file = d.export(true).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&file).unwrap()).unwrap();
        let mut exported = String::new();
        for i in 0..zip.len() {
            use std::io::Read;
            zip.by_index(i)
                .unwrap()
                .read_to_string(&mut exported)
                .unwrap();
        }
        for secret in [token, "hunter2", "hunter3"] {
            assert!(
                !exported.contains(secret),
                "{secret} leaked into the export"
            );
        }
        // The export contains exactly the previewed sections (plus README).
        for s in &preview.sections {
            assert!(
                exported.contains(&s.content),
                "{} differs from its preview",
                s.file
            );
        }
    }

    #[test]
    fn personal_paths_are_masked() {
        let Ok(profile) = std::env::var("USERPROFILE") else {
            return;
        };
        let text = format!(
            r"{profile}\AppData\Local\Devluma\x and {}",
            profile.replace('\\', "/")
        );
        let masked = mask_paths(&text);
        assert!(
            !masked.to_lowercase().contains(&profile.to_lowercase()),
            "{masked}"
        );
        assert!(
            masked.contains("%LOCALAPPDATA%") || masked.contains("%USERPROFILE%"),
            "{masked}"
        );
    }
}
