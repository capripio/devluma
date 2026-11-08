//! Simple settings per tool (ADR 0018): what the Settings tab offers, its limits and
//! defaults, and how a value is written in each tool's own syntax.
//!
//! Values travel as canonical strings: sizes in whole megabytes (or `unlimited`), integers
//! in decimal, toggles `on`/`off`, choices by their option value, time zones as names.
//! Defaults are the tool's own built-in values (checked against the shipped binaries, see
//! `docs/decisions/0018-service-configuration.md`), so "use default" is honest.

use serde::Serialize;

use super::adapter::AdapterId;
use super::config_file::{Dialect, unquote};

pub const UNLIMITED: &str = "unlimited";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceOption {
    pub value: &'static str,
    pub label: &'static str,
}

/// A special integer with its own meaning, e.g. `0` = "No limit".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Special {
    pub value: i64,
    pub label: &'static str,
}

/// How a bare number (no unit) is read, for sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BareUnit {
    Bytes,
    Kilobytes,
    /// PostgreSQL's 8 kB pages (`shared_buffers`).
    Pages8k,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SettingKind {
    /// Megabytes. `unlimited` is the tool's own "no limit" token, when it has one.
    #[serde(rename_all = "camelCase")]
    Size {
        min: u64,
        max: u64,
        unlimited: Option<&'static str>,
        #[serde(skip)]
        bare: BareUnit,
    },
    #[serde(rename_all = "camelCase")]
    Integer {
        min: i64,
        max: i64,
        unit: &'static str,
        special: Option<Special>,
    },
    Toggle,
    Choice {
        options: &'static [ChoiceOption],
    },
    Timezone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingSpec {
    /// The key in the tool's own config (or Devluma's name for a generated one).
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub group: &'static str,
    pub kind: SettingKind,
    /// Canonical value the tool uses when the setting isn't set. `None`: it depends on the
    /// machine; `default_label` says how.
    pub default: Option<&'static str>,
    pub default_label: Option<&'static str>,
}

/// When saved settings take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    /// Devluma reloads a running server after saving.
    Reload,
    /// The service (or PHP backend) must be restarted.
    Restart,
}

/// Where settings are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// In the tool's own file (the one the advanced editor shows), edited in place.
    File {
        dialect: Dialect,
        file: &'static str,
    },
    /// In Devluma's database, rendered into the config it generates before every start.
    Managed,
}

#[derive(Debug, Clone, Copy)]
pub struct Schema {
    pub store: Store,
    pub applies: Applies,
    pub specs: &'static [SettingSpec],
    /// How values are written: chooses units and quoting.
    pub syntax: Syntax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Php,
    MyCnf,
    Postgres,
    Nginx,
    Apache,
    Caddy,
    Mongo,
    Args,
}

impl Schema {
    pub fn spec(&self, key: &str) -> Option<&'static SettingSpec> {
        self.specs.iter().find(|s| s.key == key)
    }
}

const fn size(min: u64, max: u64, unlimited: Option<&'static str>) -> SettingKind {
    SettingKind::Size {
        min,
        max,
        unlimited,
        bare: BareUnit::Bytes,
    }
}

const fn int(min: i64, max: i64, unit: &'static str) -> SettingKind {
    SettingKind::Integer {
        min,
        max,
        unit,
        special: None,
    }
}

const fn int_special(
    min: i64,
    max: i64,
    unit: &'static str,
    value: i64,
    label: &'static str,
) -> SettingKind {
    SettingKind::Integer {
        min,
        max,
        unit,
        special: Some(Special { value, label }),
    }
}

const fn spec(
    key: &'static str,
    label: &'static str,
    group: &'static str,
    kind: SettingKind,
    default: &'static str,
    help: &'static str,
) -> SettingSpec {
    SettingSpec {
        key,
        label,
        help,
        group,
        kind,
        default: Some(default),
        default_label: None,
    }
}

// ---- PHP (php.ini) -------------------------------------------------------------------------

const PHP_SPECS: &[SettingSpec] = &[
    spec(
        "memory_limit",
        "Memory limit",
        "Resources",
        size(16, 65_536, Some("-1")),
        "128",
        "Most memory one script may use. Composer and large imports often need more.",
    ),
    spec(
        "max_execution_time",
        "Time limit",
        "Resources",
        int_special(0, 86_400, "seconds", 0, "No limit"),
        "30",
        "How long a web request may run. Command-line PHP has no limit regardless.",
    ),
    spec(
        "max_input_vars",
        "Form fields",
        "Resources",
        int(100, 1_000_000, "fields"),
        "1000",
        "Most input variables one request may send. Large admin forms may need more.",
    ),
    spec(
        "upload_max_filesize",
        "Largest upload",
        "Uploads",
        size(1, 65_536, None),
        "2",
        "Largest single uploaded file.",
    ),
    spec(
        "post_max_size",
        "Largest request",
        "Uploads",
        size(1, 65_536, Some("0")),
        "8",
        "Largest whole request body. Keep it at least as large as the largest upload.",
    ),
    spec(
        "max_file_uploads",
        "Files per request",
        "Uploads",
        int(1, 1_000, "files"),
        "20",
        "Most files one request may upload.",
    ),
    spec(
        "display_errors",
        "Show errors",
        "Errors",
        SettingKind::Toggle,
        "on",
        "Print errors in the page. Useful while developing; never on a public server.",
    ),
    spec(
        "log_errors",
        "Log errors",
        "Errors",
        SettingKind::Toggle,
        "off",
        "Write errors to the error log.",
    ),
    spec(
        "error_reporting",
        "Report",
        "Errors",
        SettingKind::Choice {
            options: &[
                ChoiceOption {
                    value: "E_ALL",
                    label: "Everything",
                },
                ChoiceOption {
                    value: "E_ALL & ~E_DEPRECATED",
                    label: "Everything except deprecations",
                },
                ChoiceOption {
                    value: "E_ALL & ~E_DEPRECATED & ~E_NOTICE",
                    label: "Errors and warnings only",
                },
            ],
        },
        "E_ALL",
        "Which problems PHP reports.",
    ),
    spec(
        "date.timezone",
        "Time zone",
        "Date and time",
        SettingKind::Timezone,
        "UTC",
        "Default time zone for date functions.",
    ),
];

// ---- Nginx (generated nginx.conf) ------------------------------------------------------------

const NGINX_SPECS: &[SettingSpec] = &[
    spec(
        "client_max_body_size",
        "Largest request",
        "Requests",
        size(1, 65_536, Some("0")),
        "1",
        "Largest request body, such as an upload. Larger requests get “413 Request Entity Too Large”.",
    ),
    spec(
        "fastcgi_read_timeout",
        "PHP response timeout",
        "Requests",
        int(1, 86_400, "seconds"),
        "60",
        "How long nginx waits for PHP to answer. Raise it for long requests or step debugging.",
    ),
    spec(
        "keepalive_timeout",
        "Keep-alive timeout",
        "Connections",
        int_special(0, 3_600, "seconds", 0, "Off"),
        "75",
        "How long an idle browser connection stays open.",
    ),
    spec(
        "worker_connections",
        "Connections",
        "Connections",
        int(64, 16_384, "connections"),
        "1024",
        "Most simultaneous connections.",
    ),
    spec(
        "gzip",
        "Compress responses",
        "Performance",
        SettingKind::Toggle,
        "off",
        "Gzip text responses (HTML only unless you add types in custom.conf).",
    ),
];

// ---- Apache (generated httpd.conf) ---------------------------------------------------------

const APACHE_SPECS: &[SettingSpec] = &[
    spec(
        "LimitRequestBody",
        "Largest request",
        "Requests",
        size(1, 2_047, Some("0")),
        "1024",
        "Largest request body, such as an upload.",
    ),
    spec(
        "Timeout",
        "Timeout",
        "Requests",
        int(1, 86_400, "seconds"),
        "60",
        "How long Apache waits for a request or for PHP to answer.",
    ),
    spec(
        "KeepAlive",
        "Keep-alive",
        "Connections",
        SettingKind::Toggle,
        "on",
        "Reuse a browser connection for several requests.",
    ),
    spec(
        "KeepAliveTimeout",
        "Keep-alive timeout",
        "Connections",
        int(1, 3_600, "seconds"),
        "5",
        "How long an idle connection stays open.",
    ),
    spec(
        "ThreadsPerChild",
        "Worker threads",
        "Connections",
        int(1, 1_920, "threads"),
        "64",
        "Simultaneous requests Apache can handle.",
    ),
];

// ---- Caddy (generated Caddyfile) -----------------------------------------------------------
//
// Defaults read from the v2.11.9 source: `request_body` limits only when `MaxSize > 0`;
// the FastCGI transport sets a read deadline only when `ReadTimeout != 0`;
// `defaultIdleTimeout = 5 * time.Minute`; nothing is compressed without `encode`.

const CADDY_SPECS: &[SettingSpec] = &[
    spec(
        "max_size",
        "Largest request",
        "Requests",
        size(1, 65_536, Some("0")),
        UNLIMITED,
        "Largest request body, such as an upload. Larger requests get “413 Request Entity Too Large”.",
    ),
    spec(
        "php_read_timeout",
        "PHP response timeout",
        "Requests",
        int_special(1, 86_400, "seconds", 0, "No limit"),
        "0",
        "How long Caddy waits for PHP to answer.",
    ),
    spec(
        "idle_timeout",
        "Keep-alive timeout",
        "Connections",
        int(1, 86_400, "seconds"),
        "300",
        "How long an idle browser connection stays open.",
    ),
    spec(
        "encode",
        "Compress responses",
        "Performance",
        SettingKind::Toggle,
        "off",
        "Compress responses with zstd or gzip, whichever the browser supports.",
    ),
];

/// The Caddy settings that apply to FrankenPHP (same defaults: it is Caddy).
const FRANKENPHP_SPECS: &[SettingSpec] = &[CADDY_SPECS[0], CADDY_SPECS[2], CADDY_SPECS[3]];

// ---- MariaDB (my.ini, [mysqld]) ------------------------------------------------------------

const MARIADB_SPECS: &[SettingSpec] = &[
    spec(
        "max_connections",
        "Connections",
        "Connections",
        int(10, 100_000, "connections"),
        "151",
        "Most simultaneous client connections.",
    ),
    spec(
        "max_allowed_packet",
        "Largest packet",
        "Connections",
        size(1, 1_024, None),
        "16",
        "Largest single query or row. Raise it to import big dumps or store large blobs.",
    ),
    spec(
        "innodb_buffer_pool_size",
        "Buffer pool",
        "Memory",
        size(32, 1_048_576, None),
        "128",
        "Memory InnoDB uses to cache tables and indexes. The biggest speed setting.",
    ),
    spec(
        "character_set_server",
        "Character set",
        "Text",
        SettingKind::Choice {
            options: &[
                ChoiceOption {
                    value: "utf8mb4",
                    label: "utf8mb4 (full Unicode, emoji)",
                },
                ChoiceOption {
                    value: "utf8mb3",
                    label: "utf8mb3 (legacy UTF-8)",
                },
                ChoiceOption {
                    value: "latin1",
                    label: "latin1",
                },
            ],
        },
        "utf8mb4",
        "Default for new databases and tables.",
    ),
    spec(
        "slow_query_log",
        "Slow query log",
        "Logging",
        SettingKind::Toggle,
        "off",
        "Log queries slower than the threshold below (to <computer>-slow.log in the data folder).",
    ),
    spec(
        "long_query_time",
        "Slow query threshold",
        "Logging",
        int(0, 3_600, "seconds"),
        "10",
        "A query taking longer than this counts as slow.",
    ),
];

// ---- PostgreSQL (postgresql.conf) ----------------------------------------------------------

const POSTGRES_SPECS: &[SettingSpec] = &[
    spec(
        "max_connections",
        "Connections",
        "Connections",
        int(5, 10_000, "connections"),
        "100",
        "Most simultaneous client connections.",
    ),
    spec(
        "shared_buffers",
        "Shared buffers",
        "Memory",
        SettingKind::Size {
            min: 1,
            max: 1_048_576,
            unlimited: None,
            bare: BareUnit::Pages8k,
        },
        "128",
        "Memory PostgreSQL uses to cache data.",
    ),
    spec(
        "work_mem",
        "Work memory",
        "Memory",
        SettingKind::Size {
            min: 1,
            max: 1_048_576,
            unlimited: None,
            bare: BareUnit::Kilobytes,
        },
        "4",
        "Memory per sort or hash step of a query, before it spills to disk.",
    ),
    spec(
        "maintenance_work_mem",
        "Maintenance memory",
        "Memory",
        SettingKind::Size {
            min: 1,
            max: 1_048_576,
            unlimited: None,
            bare: BareUnit::Kilobytes,
        },
        "64",
        "Memory for VACUUM, CREATE INDEX and restores.",
    ),
    spec(
        "log_statement",
        "Log statements",
        "Logging",
        SettingKind::Choice {
            options: &[
                ChoiceOption {
                    value: "none",
                    label: "None",
                },
                ChoiceOption {
                    value: "ddl",
                    label: "Schema changes",
                },
                ChoiceOption {
                    value: "mod",
                    label: "Schema and data changes",
                },
                ChoiceOption {
                    value: "all",
                    label: "All statements",
                },
            ],
        },
        "none",
        "Which SQL statements go to the server log.",
    ),
    spec(
        "log_min_duration_statement",
        "Slow query threshold",
        "Logging",
        int_special(-1, 3_600_000, "ms", -1, "Off"),
        "-1",
        "Log statements that run at least this long.",
    ),
];

// ---- MongoDB (generated mongod.conf) ---------------------------------------------------------

const MONGO_SPECS: &[SettingSpec] = &[
    SettingSpec {
        key: "cache_size",
        label: "Cache size",
        help: "Memory WiredTiger uses for its cache.",
        group: "Memory",
        kind: size(256, 1_048_576, None),
        default: None,
        default_label: Some("Half of the computer's memory minus 1 GB"),
    },
    spec(
        "slow_op_threshold_ms",
        "Slow operation threshold",
        "Logging",
        int(0, 3_600_000, "ms"),
        "100",
        "Operations slower than this are logged.",
    ),
    spec(
        "verbosity",
        "Log detail",
        "Logging",
        SettingKind::Choice {
            options: &[
                ChoiceOption {
                    value: "0",
                    label: "Normal",
                },
                ChoiceOption {
                    value: "1",
                    label: "Debug 1",
                },
                ChoiceOption {
                    value: "2",
                    label: "Debug 2",
                },
                ChoiceOption {
                    value: "3",
                    label: "Debug 3",
                },
            ],
        },
        "0",
        "How much mongod writes to its log.",
    ),
];

// ---- Mailpit (command-line options) ----------------------------------------------------------

const MAILPIT_SPECS: &[SettingSpec] = &[spec(
    "max",
    "Messages kept",
    "Storage",
    int_special(0, 1_000_000, "messages", 0, "No limit"),
    "500",
    "Older messages are deleted beyond this number.",
)];

pub fn schema(adapter: AdapterId) -> Option<Schema> {
    let s = |store, applies, specs, syntax| {
        Some(Schema {
            store,
            applies,
            specs,
            syntax,
        })
    };
    match adapter {
        AdapterId::Php => s(
            Store::File {
                dialect: Dialect::Php,
                file: "php.ini",
            },
            Applies::Restart,
            PHP_SPECS,
            Syntax::Php,
        ),
        AdapterId::Nginx => s(Store::Managed, Applies::Reload, NGINX_SPECS, Syntax::Nginx),
        AdapterId::Apache => s(
            Store::Managed,
            Applies::Reload,
            APACHE_SPECS,
            Syntax::Apache,
        ),
        AdapterId::Mariadb => s(
            Store::File {
                dialect: Dialect::MyCnf,
                file: "my.ini",
            },
            Applies::Restart,
            MARIADB_SPECS,
            Syntax::MyCnf,
        ),
        AdapterId::Postgresql => s(
            Store::File {
                dialect: Dialect::Postgres,
                file: "postgresql.conf",
            },
            Applies::Restart,
            POSTGRES_SPECS,
            Syntax::Postgres,
        ),
        AdapterId::Mongodb => s(Store::Managed, Applies::Restart, MONGO_SPECS, Syntax::Mongo),
        AdapterId::Mailpit => s(
            Store::Managed,
            Applies::Restart,
            MAILPIT_SPECS,
            Syntax::Args,
        ),
        AdapterId::Caddy => s(Store::Managed, Applies::Reload, CADDY_SPECS, Syntax::Caddy),
        // FrankenPHP: no FastCGI timeout (PHP runs inside the server). Restart, because its
        // php.ini and PHP threads are only read at start.
        AdapterId::Frankenphp => s(
            Store::Managed,
            Applies::Restart,
            FRANKENPHP_SPECS,
            Syntax::Caddy,
        ),
        AdapterId::Node | AdapterId::Python | AdapterId::Cmder => None,
        #[cfg(test)]
        AdapterId::Fixture => s(
            Store::Managed,
            Applies::Restart,
            tests::FIXTURE_SPECS,
            Syntax::Args,
        ),
    }
}

// ---- values --------------------------------------------------------------------------------

/// Checks a canonical value against its spec and returns it normalized.
pub fn validate(spec: &SettingSpec, value: &str) -> Result<String, String> {
    let v = value.trim();
    let bad = |why: String| Err(format!("{}: {why}", spec.label));
    match spec.kind {
        SettingKind::Size {
            min,
            max,
            unlimited,
            ..
        } => {
            if v == UNLIMITED {
                return if unlimited.is_some() {
                    Ok(UNLIMITED.into())
                } else {
                    bad("there's no unlimited setting for this.".into())
                };
            }
            match v.parse::<u64>() {
                Ok(n) if (min..=max).contains(&n) => Ok(n.to_string()),
                _ => bad(format!("enter a whole number of MB from {min} to {max}.")),
            }
        }
        SettingKind::Integer {
            min, max, special, ..
        } => match v.parse::<i64>() {
            Ok(n) if (min..=max).contains(&n) || special.is_some_and(|s| s.value == n) => {
                Ok(n.to_string())
            }
            _ => bad(format!("enter a whole number from {min} to {max}.")),
        },
        SettingKind::Toggle => match v {
            "on" | "off" => Ok(v.into()),
            _ => bad("choose on or off.".into()),
        },
        SettingKind::Choice { options } => {
            if options.iter().any(|o| o.value == v) {
                Ok(v.into())
            } else {
                bad("choose one of the listed options.".into())
            }
        }
        SettingKind::Timezone => {
            let ok = !v.is_empty()
                && v.len() <= 64
                && v.split('/').all(|part| {
                    !part.is_empty()
                        && part
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
                });
            if ok {
                Ok(v.into())
            } else {
                bad("use a time zone name such as Europe/London or UTC.".into())
            }
        }
    }
}

/// Reads a size in the tool's syntax as whole megabytes. `None` if it isn't one.
fn parse_size_mb(raw: &str, bare: BareUnit) -> Option<u64> {
    let v = raw.trim();
    let digits_end = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
    let (num, unit) = v.split_at(digits_end);
    let n: u64 = num.parse().ok()?;
    let kb: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" => match bare {
            BareUnit::Bytes => {
                return n.is_multiple_of(1024 * 1024).then_some(n / (1024 * 1024));
            }
            BareUnit::Kilobytes => n,
            BareUnit::Pages8k => n.checked_mul(8)?,
        },
        "k" | "kb" => n,
        "m" | "mb" => n.checked_mul(1024)?,
        "g" | "gb" => n.checked_mul(1024 * 1024)?,
        "t" | "tb" => n.checked_mul(1024 * 1024 * 1024)?,
        _ => return None,
    };
    kb.is_multiple_of(1024).then_some(kb / 1024)
}

fn parse_toggle(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => Some("on"),
        "0" | "off" | "false" | "no" | "" | "none" => Some("off"),
        _ => None,
    }
}

/// Reads a value found in a config file into canonical form. `None`: Devluma can't show it
/// as a simple setting (the raw text is shown instead, and saving replaces it).
pub fn decode(spec: &SettingSpec, raw: &str) -> Option<String> {
    let v = unquote(raw).trim();
    let canonical = match spec.kind {
        SettingKind::Size {
            unlimited, bare, ..
        } => {
            if unlimited.is_some_and(|u| u == v) {
                return Some(UNLIMITED.into());
            }
            parse_size_mb(v, bare)?.to_string()
        }
        SettingKind::Integer { .. } => v.parse::<i64>().ok()?.to_string(),
        SettingKind::Toggle => parse_toggle(v)?.to_owned(),
        SettingKind::Choice { options } => {
            let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
            let found = norm(v);
            options
                .iter()
                .find(|o| norm(o.value).eq_ignore_ascii_case(&found))?
                .value
                .to_owned()
        }
        SettingKind::Timezone => v.to_owned(),
    };
    validate(spec, &canonical).ok()
}

/// Writes a canonical (validated) value in the tool's syntax.
pub fn encode(syntax: Syntax, spec: &SettingSpec, value: &str) -> String {
    match spec.kind {
        SettingKind::Size { unlimited, .. } => {
            if value == UNLIMITED {
                return unlimited.unwrap_or("0").to_owned();
            }
            let mb: u64 = value.parse().unwrap_or(0);
            match syntax {
                Syntax::Php | Syntax::MyCnf => format!("{mb}M"),
                Syntax::Postgres => format!("{mb}MB"),
                Syntax::Nginx => format!("{mb}m"),
                Syntax::Apache => (mb * 1024 * 1024).to_string(),
                // humanize.ParseBytes: "MB" would be 10^6 bytes, "MiB" is 2^20.
                Syntax::Caddy => format!("{mb}MiB"),
                Syntax::Mongo => {
                    // cacheSizeGB is a decimal number of gigabytes.
                    let gb = mb as f64 / 1024.0;
                    format!("{gb:.3}")
                        .trim_end_matches('0')
                        .trim_end_matches('.')
                        .to_owned()
                }
                Syntax::Args => mb.to_string(),
            }
        }
        SettingKind::Toggle => {
            let on = value == "on";
            match syntax {
                Syntax::Php | Syntax::Apache => if on { "On" } else { "Off" }.into(),
                Syntax::MyCnf => if on { "ON" } else { "OFF" }.into(),
                _ => if on { "on" } else { "off" }.into(),
            }
        }
        SettingKind::Choice { .. } if syntax == Syntax::Postgres => format!("'{value}'"),
        SettingKind::Timezone if syntax == Syntax::Php => format!("\"{value}\""),
        _ => value.to_owned(),
    }
}

/// "512 MB", "2 GB", "No limit", "30 seconds", "On" — for summaries of changes.
pub fn describe(spec: &SettingSpec, value: &str) -> String {
    match spec.kind {
        SettingKind::Size { .. } if value == UNLIMITED => "No limit".into(),
        SettingKind::Size { .. } => match value.parse::<u64>() {
            Ok(mb) if mb >= 1024 && mb % 1024 == 0 => format!("{} GB", mb / 1024),
            _ => format!("{value} MB"),
        },
        SettingKind::Integer { unit, special, .. } => match special {
            Some(s) if s.value.to_string() == value => s.label.into(),
            _ => format!("{value} {unit}"),
        },
        SettingKind::Toggle => if value == "on" { "On" } else { "Off" }.into(),
        SettingKind::Choice { options } => options
            .iter()
            .find(|o| o.value == value)
            .map_or_else(|| value.to_owned(), |o| o.label.to_owned()),
        SettingKind::Timezone => value.to_owned(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FIXTURE_SPECS: &[SettingSpec] = &[spec(
        "limit",
        "Limit",
        "General",
        int(1, 100, "items"),
        "10",
        "Fake setting.",
    )];

    fn php(key: &str) -> &'static SettingSpec {
        schema(AdapterId::Php).unwrap().spec(key).unwrap()
    }

    #[test]
    fn sizes_read_every_unit_and_keep_odd_values_raw() {
        let mem = php("memory_limit");
        assert_eq!(decode(mem, "128M").as_deref(), Some("128"));
        assert_eq!(decode(mem, "2G").as_deref(), Some("2048"));
        assert_eq!(decode(mem, "268435456").as_deref(), Some("256"));
        assert_eq!(decode(mem, "-1").as_deref(), Some(UNLIMITED));
        assert_eq!(decode(mem, "1500K"), None, "not whole MB");
        assert_eq!(decode(mem, "12XYZ"), None);
        assert_eq!(decode(mem, "8M"), None, "below the minimum");
        assert_eq!(encode(Syntax::Php, mem, "512"), "512M");
        assert_eq!(encode(Syntax::Php, mem, UNLIMITED), "-1");

        let pg = schema(AdapterId::Postgresql).unwrap();
        let shared = pg.spec("shared_buffers").unwrap();
        assert_eq!(
            decode(shared, "16384").as_deref(),
            Some("128"),
            "8 kB pages"
        );
        assert_eq!(decode(shared, "128MB").as_deref(), Some("128"));
        assert_eq!(decode(shared, "1GB").as_deref(), Some("1024"));
        assert_eq!(encode(Syntax::Postgres, shared, "256"), "256MB");
        let work = pg.spec("work_mem").unwrap();
        assert_eq!(decode(work, "4096").as_deref(), Some("4"), "kilobytes");

        let apache = schema(AdapterId::Apache).unwrap();
        let body = apache.spec("LimitRequestBody").unwrap();
        assert_eq!(encode(Syntax::Apache, body, "10"), "10485760");
        let caddy = schema(AdapterId::Caddy).unwrap();
        let max = caddy.spec("max_size").unwrap();
        assert_eq!(encode(Syntax::Caddy, max, "64"), "64MiB");
        assert_eq!(encode(Syntax::Caddy, max, UNLIMITED), "0");
        let mongo = schema(AdapterId::Mongodb).unwrap();
        let cache = mongo.spec("cache_size").unwrap();
        assert_eq!(encode(Syntax::Mongo, cache, "1536"), "1.5");
        assert_eq!(encode(Syntax::Mongo, cache, "2048"), "2");
    }

    #[test]
    fn values_are_validated_against_their_limits() {
        assert!(validate(php("memory_limit"), "unlimited").is_ok());
        assert!(validate(php("upload_max_filesize"), "unlimited").is_err());
        assert!(validate(php("upload_max_filesize"), "0").is_err());
        assert_eq!(validate(php("max_execution_time"), "0").unwrap(), "0");
        assert!(validate(php("max_execution_time"), "-5").is_err());
        assert!(validate(php("display_errors"), "maybe").is_err());
        assert!(validate(php("error_reporting"), "E_ALL").is_ok());
        assert!(validate(php("error_reporting"), "E_ALL; rm").is_err());
        assert!(validate(php("date.timezone"), "America/Argentina/Buenos_Aires").is_ok());
        assert!(validate(php("date.timezone"), "Etc/GMT+5").is_ok());
        assert!(validate(php("date.timezone"), "x\"\nextension=evil").is_err());
        assert!(validate(php("date.timezone"), "../x").is_err());
    }

    #[test]
    fn toggles_choices_and_descriptions() {
        assert_eq!(decode(php("display_errors"), "On").as_deref(), Some("on"));
        assert_eq!(decode(php("display_errors"), "0").as_deref(), Some("off"));
        assert_eq!(decode(php("display_errors"), "stderr"), None);
        assert_eq!(
            decode(php("error_reporting"), "E_ALL  &  ~E_DEPRECATED").as_deref(),
            Some("E_ALL & ~E_DEPRECATED")
        );
        assert_eq!(
            decode(php("date.timezone"), "\"Asia/Karachi\"").as_deref(),
            Some("Asia/Karachi")
        );
        assert_eq!(encode(Syntax::Php, php("date.timezone"), "UTC"), "\"UTC\"");
        let pg = schema(AdapterId::Postgresql).unwrap();
        assert_eq!(
            encode(Syntax::Postgres, pg.spec("log_statement").unwrap(), "ddl"),
            "'ddl'"
        );
        assert_eq!(describe(php("memory_limit"), "2048"), "2 GB");
        assert_eq!(describe(php("memory_limit"), UNLIMITED), "No limit");
        assert_eq!(describe(php("max_execution_time"), "0"), "No limit");
        assert_eq!(describe(php("max_execution_time"), "120"), "120 seconds");
    }

    #[test]
    fn every_default_is_valid_and_keys_are_unique() {
        use AdapterId::*;
        for adapter in [
            Php, Nginx, Mariadb, Node, Python, Mailpit, Postgresql, Mongodb, Apache, Caddy,
            Frankenphp, Cmder, Fixture,
        ] {
            let Some(schema) = schema(adapter) else {
                continue;
            };
            let mut keys: Vec<_> = schema.specs.iter().map(|s| s.key).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), schema.specs.len(), "{adapter:?}");
            for s in schema.specs {
                if let Some(d) = s.default {
                    assert_eq!(validate(s, d).as_deref(), Ok(d), "{adapter:?} {}", s.key);
                } else {
                    assert!(s.default_label.is_some(), "{}", s.key);
                }
            }
        }
    }
}
