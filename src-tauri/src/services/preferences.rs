//! User preferences from the Settings page (ADR 0015): domain suffix, terminal, default and
//! HTTPS ports, services to start with Devluma, download timeouts.
//!
//! Each value is one `settings` row; a missing row means the default. Values are validated
//! before they are written. A row that no longer parses (edited by hand, or written by a
//! newer build) falls back to the default instead of breaking the feature that reads it.

use rusqlite::Connection;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::domain::adapter::AdapterId;
use crate::persistence::{DbError, DbResult, services, settings};

const DOMAIN_SUFFIX: &str = "sites.domain_suffix";
const HTTPS_PORT: &str = "sites.https_port";
const TERMINAL: &str = "terminal.program";
const DEFAULT_PORTS: &str = "services.default_ports";
const START_SERVICES: &str = "startup.start_services";
const DOWNLOAD_TIMEOUTS: &str = "downloads.timeouts";

/// Suffixes suggested in Settings. Any valid suffix can be entered; these are reserved for
/// local use and the local CA can sign them.
pub const SUGGESTED_SUFFIXES: [&str; 4] = ["test", "localhost", "internal", "local"];

/// Normalizes a domain suffix typed by the user: trims spaces and dots, lowercases, converts
/// to IDNA, and checks it is a valid end of a hostname (one or more labels, e.g. `local`
/// or `dev.test`).
pub fn normalize_suffix(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_matches('.');
    if trimmed.is_empty() {
        return Err("Enter a domain suffix, for example test or local.".into());
    }
    let checked = crate::domain::domain_name::check(&format!("site.{trimmed}")).map_err(|_| {
        format!("\"{trimmed}\" isn't a valid domain suffix. Use letters, digits, hyphens and dots.")
    })?;
    let normalized = checked
        .normalized
        .strip_prefix("site.")
        .unwrap_or(&checked.normalized)
        .to_owned();
    if normalized.len() > 200 {
        return Err("The domain suffix is too long.".into());
    }
    Ok(normalized)
}

/// What a suffix means for sites that use it, shown under the Settings field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuffixAdvice {
    /// The local CA can issue certificates for it (its last label is in the CA's constraints).
    pub https_supported: bool,
    pub warnings: Vec<String>,
}

pub fn suffix_advice(suffix: &str) -> SuffixAdvice {
    use crate::services::certificates::LOCAL_SUFFIXES;
    let tld = suffix.rsplit('.').next().unwrap_or_default();
    let https_supported = LOCAL_SUFFIXES.contains(&tld);
    let mut warnings = Vec::new();
    if tld == "local" {
        warnings.push(
            ".local is also used by mDNS/Bonjour (printers, other computers), so names can resolve slowly or inconsistently. .test is recommended."
                .into(),
        );
    } else if !https_supported {
        warnings.push(format!(
            ".{tld} isn't reserved for local use. If it's a real top-level domain, this computer will stop reaching real websites with the same names, and browsers may force HTTPS for it (for example .dev and .app)."
        ));
    }
    if !https_supported {
        warnings.push(format!(
            "HTTPS won't be available for these sites: Devluma's local certificate authority can only sign {}.",
            LOCAL_SUFFIXES.map(|s| format!(".{s}")).join(", ")
        ));
    }
    SuffixAdvice {
        https_supported,
        warnings,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalProgram {
    #[default]
    PowerShell,
    CommandPrompt,
    Cmder,
}

/// Ports pre-filled for new services (Add service and the setup wizard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultPorts {
    pub nginx: u16,
    pub apache: u16,
    pub mariadb: u16,
    pub postgresql: u16,
    pub mongodb: u16,
    pub mailpit: u16,
    pub mailpit_web: u16,
}

impl Default for DefaultPorts {
    fn default() -> Self {
        Self {
            nginx: 8080,
            apache: 8081,
            mariadb: 3306,
            postgresql: 5432,
            mongodb: 27017,
            mailpit: 1025,
            mailpit_web: 8025,
        }
    }
}

impl DefaultPorts {
    /// `(port, admin port)` for a service adapter; `None` for adapters without a service.
    pub fn for_adapter(&self, adapter: AdapterId) -> Option<(u16, Option<u16>)> {
        Some(match adapter {
            AdapterId::Nginx => (self.nginx, None),
            AdapterId::Apache => (self.apache, None),
            AdapterId::Mariadb => (self.mariadb, None),
            AdapterId::Postgresql => (self.postgresql, None),
            AdapterId::Mongodb => (self.mongodb, None),
            AdapterId::Mailpit => (self.mailpit, Some(self.mailpit_web)),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTimeouts {
    /// Seconds to wait for a connection.
    pub connect_secs: u16,
    /// Seconds without any data before a download is abandoned.
    pub idle_secs: u16,
}

impl Default for DownloadTimeouts {
    fn default() -> Self {
        Self {
            connect_secs: 15,
            idle_secs: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// Without the dot, e.g. `test`.
    pub domain_suffix: String,
    /// `None` = automatic: 443 when the web server is on port 80, else 8443.
    pub https_port: Option<u16>,
    pub terminal: TerminalProgram,
    pub default_ports: DefaultPorts,
    /// Service ids started when Devluma opens.
    pub start_services: Vec<String>,
    pub download_timeouts: DownloadTimeouts,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            domain_suffix: "test".into(),
            https_port: None,
            terminal: TerminalProgram::default(),
            default_ports: DefaultPorts::default(),
            start_services: Vec::new(),
            download_timeouts: DownloadTimeouts::default(),
        }
    }
}

/// A stored value, or `None` when it is missing or no longer parses.
fn get_or_none<T: DeserializeOwned>(c: &Connection, key: &str) -> DbResult<Option<T>> {
    match settings::get::<T>(c, key) {
        Err(DbError::Json(_)) => Ok(None),
        other => other,
    }
}

pub fn load(c: &Connection) -> DbResult<Preferences> {
    let d = Preferences::default();
    let suffix = get_or_none::<String>(c, DOMAIN_SUFFIX)?
        .and_then(|s| normalize_suffix(&s).ok())
        .unwrap_or(d.domain_suffix);
    let mut p = Preferences {
        domain_suffix: suffix,
        https_port: get_or_none::<Option<u16>>(c, HTTPS_PORT)?.unwrap_or(d.https_port),
        terminal: get_or_none(c, TERMINAL)?.unwrap_or(d.terminal),
        default_ports: get_or_none(c, DEFAULT_PORTS)?.unwrap_or(d.default_ports),
        start_services: get_or_none(c, START_SERVICES)?.unwrap_or(d.start_services),
        download_timeouts: get_or_none(c, DOWNLOAD_TIMEOUTS)?.unwrap_or(d.download_timeouts),
    };
    // Each group falls back on its own if it's out of range.
    let defaults = Preferences::default();
    if validate_ports(&p).is_err() {
        p.https_port = defaults.https_port;
        p.default_ports = defaults.default_ports;
    }
    if validate_timeouts(&p.download_timeouts).is_err() {
        p.download_timeouts = defaults.download_timeouts;
    }
    Ok(p)
}

/// Checks everything that doesn't need other tables (`save` checks service ids).
pub fn validate(p: &Preferences) -> Result<(), String> {
    normalize_suffix(&p.domain_suffix)?;
    validate_ports(p)?;
    validate_timeouts(&p.download_timeouts)
}

fn validate_ports(p: &Preferences) -> Result<(), String> {
    let d = &p.default_ports;
    let all = [
        ("Nginx", d.nginx),
        ("Apache", d.apache),
        ("MariaDB", d.mariadb),
        ("PostgreSQL", d.postgresql),
        ("MongoDB", d.mongodb),
        ("Mailpit SMTP", d.mailpit),
        ("Mailpit web interface", d.mailpit_web),
    ];
    if let Some((name, _)) = all.iter().find(|(_, port)| *port == 0) {
        return Err(format!("Choose a port between 1 and 65535 for {name}."));
    }
    if d.mailpit == d.mailpit_web {
        return Err("Mailpit's SMTP and web interface ports must be different.".into());
    }
    if p.https_port == Some(0) {
        return Err("Choose an HTTPS port between 1 and 65535, or Automatic.".into());
    }
    Ok(())
}

fn validate_timeouts(t: &DownloadTimeouts) -> Result<(), String> {
    if !(5..=300).contains(&t.connect_secs) {
        return Err("The connection timeout must be between 5 and 300 seconds.".into());
    }
    if !(10..=600).contains(&t.idle_secs) {
        return Err("The stalled-download timeout must be between 10 and 600 seconds.".into());
    }
    Ok(())
}

/// Validates and stores everything in one transaction (`c` is the `write` transaction). The
/// domain suffix is stored normalized.
pub fn save(c: &Connection, p: &Preferences) -> DbResult<()> {
    validate(p).map_err(DbError::Invariant)?;
    let suffix = normalize_suffix(&p.domain_suffix).map_err(DbError::Invariant)?;
    let known = services::list(c)?;
    if let Some(id) = p
        .start_services
        .iter()
        .find(|id| !known.iter().any(|s| &s.id == *id))
    {
        return Err(DbError::Invariant(format!(
            "Service {id} doesn't exist any more. Reload the page and try again."
        )));
    }
    settings::set(c, DOMAIN_SUFFIX, &suffix)?;
    settings::set(c, HTTPS_PORT, &p.https_port)?;
    settings::set(c, TERMINAL, &p.terminal)?;
    settings::set(c, DEFAULT_PORTS, &p.default_ports)?;
    settings::set(c, START_SERVICES, &p.start_services)?;
    settings::set(c, DOWNLOAD_TIMEOUTS, &p.download_timeouts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    #[test]
    fn defaults_when_nothing_is_stored() {
        let db = db();
        let p = db.read(load).unwrap();
        assert_eq!(p, Preferences::default());
        assert_eq!(p.domain_suffix, "test");
        assert_eq!(
            p.default_ports.for_adapter(AdapterId::Mailpit),
            Some((1025, Some(8025)))
        );
        assert_eq!(p.default_ports.for_adapter(AdapterId::Php), None);
    }

    #[test]
    fn saves_and_loads_every_value() {
        let db = db();
        let p = Preferences {
            domain_suffix: "local".into(),
            https_port: Some(9443),
            terminal: TerminalProgram::Cmder,
            default_ports: DefaultPorts {
                nginx: 80,
                ..DefaultPorts::default()
            },
            start_services: vec![],
            download_timeouts: DownloadTimeouts {
                connect_secs: 60,
                idle_secs: 120,
            },
        };
        db.write(|c| save(c, &p)).unwrap();
        assert_eq!(db.read(load).unwrap(), p);
    }

    #[test]
    fn rejects_invalid_values_and_keeps_the_old_ones() {
        let db = db();
        let bad = [
            Preferences {
                domain_suffix: "bad_suffix".into(),
                ..Preferences::default()
            },
            Preferences {
                domain_suffix: " . ".into(),
                ..Preferences::default()
            },
            Preferences {
                https_port: Some(0),
                ..Preferences::default()
            },
            Preferences {
                default_ports: DefaultPorts {
                    mailpit_web: 1025,
                    ..DefaultPorts::default()
                },
                ..Preferences::default()
            },
            Preferences {
                default_ports: DefaultPorts {
                    mariadb: 0,
                    ..DefaultPorts::default()
                },
                ..Preferences::default()
            },
            Preferences {
                download_timeouts: DownloadTimeouts {
                    connect_secs: 1,
                    idle_secs: 30,
                },
                ..Preferences::default()
            },
            Preferences {
                start_services: vec!["no-such-service".into()],
                ..Preferences::default()
            },
        ];
        for p in bad {
            assert!(
                matches!(db.write(|c| save(c, &p)), Err(DbError::Invariant(_))),
                "{p:?} should be rejected"
            );
        }
        assert_eq!(db.read(load).unwrap(), Preferences::default());
    }

    #[test]
    fn unreadable_or_out_of_range_rows_fall_back_to_defaults() {
        let db = db();
        db.write(|c| {
            settings::set(c, DOMAIN_SUFFIX, &"not a suffix")?;
            settings::set(c, TERMINAL, &"bash")?;
            settings::set(c, DEFAULT_PORTS, &serde_json::json!({"nginx": 0}))?;
            settings::set(
                c,
                DOWNLOAD_TIMEOUTS,
                &DownloadTimeouts {
                    connect_secs: 0,
                    idle_secs: 0,
                },
            )
        })
        .unwrap();
        assert_eq!(db.read(load).unwrap(), Preferences::default());
    }
}

#[cfg(test)]
mod suffix_tests {
    use super::*;

    #[test]
    fn normalizes_any_valid_suffix() {
        assert_eq!(normalize_suffix(".Local").unwrap(), "local");
        assert_eq!(normalize_suffix(" dev.TEST. ").unwrap(), "dev.test");
        assert_eq!(normalize_suffix("lan").unwrap(), "lan");
        assert_eq!(normalize_suffix("bücher").unwrap(), "xn--bcher-kva");
        for bad in ["", " . ", "a b", "x/y", "under_score", "-bad", "a..b"] {
            assert!(normalize_suffix(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn advice_explains_https_and_shadowing() {
        assert_eq!(
            suffix_advice("test"),
            SuffixAdvice {
                https_supported: true,
                warnings: vec![]
            }
        );
        assert!(suffix_advice("dev.test").https_supported);
        let local = suffix_advice("local");
        assert!(local.https_supported);
        assert!(local.warnings[0].contains("mDNS"));
        let dev = suffix_advice("dev");
        assert!(!dev.https_supported);
        assert!(dev.warnings[0].contains("isn't reserved for local use"));
        assert!(dev.warnings[1].contains("HTTPS won't be available"));
    }

    #[test]
    fn custom_suffixes_are_stored_normalized() {
        let db = crate::persistence::test_support::db();
        let p = Preferences {
            domain_suffix: ".MyLan.".into(),
            ..Preferences::default()
        };
        db.write(|c| save(c, &p)).unwrap();
        assert_eq!(db.read(load).unwrap().domain_suffix, "mylan");
    }
}
