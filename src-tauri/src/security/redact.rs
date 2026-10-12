//! Secret redaction for anything that leaves memory: logs, operation events, process
//! output, error details and diagnostic exports (trd.md §10).
//!
//! Two layers:
//! 1. Exact values registered with [`register_secret`] (e.g. a token just read from
//!    Credential Manager) are always replaced.
//! 2. Heuristic patterns catch common shapes: `password=…`, `"apiKey": "…"`,
//!    `--password …`, `Authorization: Bearer …`, and `scheme://user:pass@host`.
//!
//! Redaction errs on the side of over-redacting; it is a safety net, not a licence to
//! pass secrets around.

use std::borrow::Cow;
use std::sync::{LazyLock, RwLock};

use regex::{Captures, Regex};

pub const MASK: &str = "[REDACTED]";

/// Shorter values would cause too many false replacements.
const MIN_SECRET_LEN: usize = 4;

static KNOWN_SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Remembers a secret value for the lifetime of the process so it is masked everywhere.
pub fn register_secret(value: &str) {
    if value.len() < MIN_SECRET_LEN {
        return;
    }
    let mut known = KNOWN_SECRETS.write().unwrap_or_else(|e| e.into_inner());
    if !known.iter().any(|k| k == value) {
        known.push(value.to_owned());
        // Longest first so a secret containing another is masked whole.
        known.sort_by_key(|k| std::cmp::Reverse(k.len()));
    }
}

const KEY: &str = r"[A-Za-z0-9_.\-]*(?:password|passwd|pwd|secret|token|api[_\-]?key|private[_\-]?key|credential)[A-Za-z0-9_\-]*";
const VALUE: &str = r#""(?:[^"\\]|\\.)*"|'[^']*'|[^\s,;&}\]"']+"#;

static URL_USERINFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://)([^/\s:@]+):([^/\s@]+)@").unwrap());
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authorization\s*:\s*)(?:(bearer|basic|token|digest)\s+)?[^\s,;]+").unwrap()
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9\-._~+/]+=*").unwrap());
static CLI_FLAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"(?i)(--?{KEY})(=|\s+)({VALUE})"#)).unwrap());
static KEY_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"(?i)\b({KEY})(["']?\s*[:=]\s*)({VALUE})"#)).unwrap());

fn mask_value(value: &str) -> String {
    match value.chars().next() {
        Some(q @ ('"' | '\'')) => format!("{q}{MASK}{q}"),
        _ => MASK.to_owned(),
    }
}

/// Applies one pattern, keeping the input borrowed when nothing matched.
fn apply<'a>(text: Cow<'a, str>, re: &Regex, f: &dyn Fn(&Captures) -> String) -> Cow<'a, str> {
    let replaced = match re.replace_all(&text, |c: &Captures| f(c)) {
        Cow::Owned(s) => Some(s),
        Cow::Borrowed(_) => None,
    };
    match replaced {
        Some(s) => Cow::Owned(s),
        None => text,
    }
}

/// Returns `input` with secrets masked. Borrows when nothing changed.
pub fn redact(input: &str) -> Cow<'_, str> {
    let mut out: Cow<'_, str> = Cow::Borrowed(input);

    {
        let known = KNOWN_SECRETS.read().unwrap_or_else(|e| e.into_inner());
        for secret in known.iter() {
            if out.contains(secret.as_str()) {
                out = Cow::Owned(out.replace(secret.as_str(), MASK));
            }
        }
    }

    out = apply(out, &URL_USERINFO, &|c| {
        format!("{}{}:{MASK}@", &c[1], &c[2])
    });
    out = apply(out, &AUTH_HEADER, &|c| match c.get(2) {
        Some(scheme) => format!("{}{} {MASK}", &c[1], scheme.as_str()),
        None => format!("{}{MASK}", &c[1]),
    });
    out = apply(out, &BEARER, &|_| format!("Bearer {MASK}"));
    // Already-masked values are kept as-is so redaction is idempotent.
    let mask_kv = |c: &Captures| {
        if c[3].contains("[REDACTED") {
            c[0].to_owned()
        } else {
            format!("{}{}{}", &c[1], &c[2], mask_value(&c[3]))
        }
    };
    out = apply(out, &CLI_FLAG, &mask_kv);
    out = apply(out, &KEY_VALUE, &mask_kv);
    out
}

/// Redacts every string inside a JSON value (keys are left alone; values under
/// secret-looking keys are masked entirely).
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    static SECRET_KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(&format!("(?i)^{KEY}$")).unwrap());
    match value {
        Value::String(s) => Value::String(redact(s).into_owned()),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let v = if SECRET_KEY.is_match(k) && !v.is_null() {
                        Value::String(MASK.to_owned())
                    } else {
                        redact_json(v)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_ordinary_text_alone() {
        let s = "Downloading php-8.4.1-nts-Win32-vs17-x64.zip (31 MB) to C:\\Devluma\\downloads";
        assert!(matches!(redact(s), Cow::Borrowed(_)));
    }

    #[test]
    fn masks_key_value_pairs() {
        assert_eq!(
            redact("MYSQL_ROOT_PASSWORD=hunter22"),
            "MYSQL_ROOT_PASSWORD=[REDACTED]"
        );
        assert_eq!(
            redact("db_password: s3cr3t, user: root"),
            "db_password: [REDACTED], user: root"
        );
        assert_eq!(
            redact(r#"{"apiKey": "abc123", "port": 8080}"#),
            r#"{"apiKey": "[REDACTED]", "port": 8080}"#
        );
    }

    #[test]
    fn masks_cli_flags() {
        assert_eq!(
            redact("mariadb-admin --user=root --password=hunter22 shutdown"),
            "mariadb-admin --user=root --password=[REDACTED] shutdown"
        );
        assert_eq!(
            redact("ngrok config add-authtoken 2abcDEF --authtoken XYZ987"),
            "ngrok config add-authtoken [REDACTED] --authtoken [REDACTED]"
        );
    }

    #[test]
    fn is_idempotent() {
        let once = redact("--password=hunter22 token: abc Authorization: Bearer x.y mysql://u:p@h")
            .into_owned();
        assert_eq!(redact(&once), once);
    }

    #[test]
    fn masks_auth_headers_and_bearer_tokens() {
        assert_eq!(
            redact("Authorization: Bearer eyJhbGciOi.payload.sig"),
            "Authorization: Bearer [REDACTED]"
        );
        assert_eq!(redact("got bearer abc.def-ghi"), "got Bearer [REDACTED]");
    }

    #[test]
    fn masks_url_credentials() {
        assert_eq!(
            redact("connect mysql://root:hunter22@127.0.0.1:3306/app"),
            "connect mysql://root:[REDACTED]@127.0.0.1:3306/app"
        );
    }

    #[test]
    fn masks_registered_secret_values_anywhere() {
        register_secret("tok_9f8e7d6c5b4a");
        assert_eq!(
            redact("tunnel failed: invalid credentials tok_9f8e7d6c5b4a for user"),
            "tunnel failed: invalid credentials [REDACTED] for user"
        );
        register_secret("abc"); // too short: ignored rather than masking every "abc"
        assert_eq!(redact("abcdef"), "abcdef");
    }

    #[test]
    fn redacts_json_values_recursively() {
        let v = serde_json::json!({
            "args": ["--password=hunter22", "--port=3306"],
            "env": { "NGROK_AUTHTOKEN": "xyz", "PATH": "C:\\php" },
            "secret": null
        });
        assert_eq!(
            redact_json(&v),
            serde_json::json!({
                "args": ["--password=[REDACTED]", "--port=3306"],
                "env": { "NGROK_AUTHTOKEN": "[REDACTED]", "PATH": "C:\\php" },
                "secret": null
            })
        );
    }
}
