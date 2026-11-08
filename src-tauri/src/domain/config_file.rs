//! Line-preserving edits of INI-style configuration files (ADR 0018).
//!
//! Simple settings are written into the same file the advanced editor shows, so the two can
//! never disagree. An edit changes only the line holding the key: everything else (comments,
//! order, blank lines, line endings) stays byte-for-byte as the user left it.
//!
//! - **php.ini**: `key = value`, `;` comments, the last active line wins. Sections are only
//!   labels, except `[PATH=…]`/`[HOST=…]`, so new keys go before the first of those.
//! - **my.ini**: `key=value` inside `[mysqld]` (or `[server]`/`[mariadb]`), `#`/`;` comments,
//!   `-` and `_` are the same in option names.
//! - **postgresql.conf**: `key = value  # comment`, the last active line wins; the commented
//!   defaults initdb writes (`#max_connections = 100`) are uncommented in place.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Php,
    MyCnf,
    Postgres,
}

/// Sections whose keys apply to the MariaDB server.
const SERVER_SECTIONS: &[&str] = &["mysqld", "server", "mariadb"];

impl Dialect {
    fn is_comment(self, c: char) -> bool {
        match self {
            Self::Php => c == ';',
            Self::MyCnf => c == '#' || c == ';',
            Self::Postgres => c == '#',
        }
    }

    fn key_eq(self, a: &str, b: &str) -> bool {
        match self {
            Self::Php => a == b,
            Self::MyCnf => a
                .replace('-', "_")
                .eq_ignore_ascii_case(&b.replace('-', "_")),
            Self::Postgres => a.eq_ignore_ascii_case(b),
        }
    }

    fn format(self, key: &str, value: &str) -> String {
        match self {
            Self::MyCnf => format!("{key}={value}"),
            Self::Php | Self::Postgres => format!("{key} = {value}"),
        }
    }
}

/// One parsed `key = value` line.
struct Entry {
    key: String,
    value: String,
    /// `;key = value` / `#key = value`.
    commented: bool,
}

fn parse_line(dialect: Dialect, line: &str) -> Option<Entry> {
    let trimmed = line.trim_start();
    let (commented, body) = match trimmed.chars().next() {
        Some(c) if dialect.is_comment(c) => (true, trimmed[c.len_utf8()..].trim_start()),
        _ => (false, trimmed),
    };
    let (key, rest) = body.split_once('=')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return None;
    }
    let mut value = rest.trim();
    // Trailing comments: postgresql.conf and my.ini (`#`), never inside quotes.
    if dialect != Dialect::Php
        && let Some(i) = unquoted_comment(value)
    {
        value = value[..i].trim_end();
    }
    Some(Entry {
        key: key.to_owned(),
        value: value.to_owned(),
        commented,
    })
}

fn unquoted_comment(value: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in value.char_indices() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') => return Some(i),
            _ => {}
        }
    }
    None
}

/// Strips one level of matching quotes.
pub fn unquote(value: &str) -> &str {
    let v = value.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v
}

fn section_name(line: &str) -> Option<&str> {
    let t = line.trim();
    t.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

/// Lines with their terminators, so a rewrite keeps the file's own line endings.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Whether line `i` applies to the server (my.ini) or is outside a scoped section (php.ini).
fn in_scope(dialect: Dialect, section: Option<&str>) -> bool {
    match dialect {
        Dialect::MyCnf => section.is_some_and(|s| {
            SERVER_SECTIONS
                .iter()
                .any(|name| s.eq_ignore_ascii_case(name))
        }),
        Dialect::Php => !section.is_some_and(is_php_scoped_section),
        Dialect::Postgres => true,
    }
}

fn is_php_scoped_section(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("PATH=") || upper.starts_with("HOST=")
}

/// The effective value of `key`: the last active line that sets it.
pub fn get(text: &str, dialect: Dialect, key: &str) -> Option<String> {
    let mut section: Option<&str> = None;
    let mut found = None;
    for line in text.lines() {
        if let Some(name) = section_name(line) {
            section = Some(name);
            continue;
        }
        if !in_scope(dialect, section) {
            continue;
        }
        if let Some(e) = parse_line(dialect, line)
            && !e.commented
            && dialect.key_eq(&e.key, key)
        {
            found = Some(e.value);
        }
    }
    found
}

/// Every active value of a repeatable key (php.ini `extension=`), in file order.
pub fn get_all(text: &str, dialect: Dialect, key: &str) -> Vec<String> {
    let mut section: Option<&str> = None;
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(name) = section_name(line) {
            section = Some(name);
            continue;
        }
        if !in_scope(dialect, section) {
            continue;
        }
        if let Some(e) = parse_line(dialect, line)
            && !e.commented
            && dialect.key_eq(&e.key, key)
        {
            out.push(e.value);
        }
    }
    out
}

/// Sets `key` to `value`, or with `None` comments out every active line for it (the tool's
/// default then applies).
///
/// Setting replaces the last active line (earlier duplicates are commented out, since they
/// no longer apply), else uncomments the last commented-out line for the key, else adds the
/// line where it applies: the end of `[mysqld]`, before php.ini's first scoped section, or
/// at the end of the file.
pub fn set(text: &str, dialect: Dialect, key: &str, value: Option<&str>) -> String {
    let nl = newline_of(text);
    let mut lines: Vec<String> = split_lines(text).into_iter().map(str::to_owned).collect();

    let mut section: Option<String> = None;
    let mut active = Vec::new();
    let mut commented = Vec::new();
    // Where a new line goes: after the last line of the first server section (my.ini) or
    // before the first scoped section (php.ini).
    let mut insert_at: Option<usize> = None;
    let mut seen_server_section = false;
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = section_name(line) {
            if dialect == Dialect::MyCnf && seen_server_section && insert_at.is_none() {
                insert_at = Some(last_content_line(&lines, i) + 1);
            }
            if dialect == Dialect::Php && is_php_scoped_section(name) && insert_at.is_none() {
                insert_at = Some(i);
            }
            section = Some(name.to_owned());
            if dialect == Dialect::MyCnf && in_scope(dialect, section.as_deref()) {
                seen_server_section = true;
            }
            continue;
        }
        if !in_scope(dialect, section.as_deref()) {
            continue;
        }
        if let Some(e) = parse_line(dialect, line)
            && dialect.key_eq(&e.key, key)
        {
            if e.commented {
                commented.push(i);
            } else {
                active.push(i);
            }
        }
    }

    let terminator = |line: &str| -> &'static str {
        if line.ends_with("\r\n") {
            "\r\n"
        } else if line.ends_with('\n') {
            "\n"
        } else {
            ""
        }
    };
    let comment_char = match dialect {
        Dialect::Php => ';',
        Dialect::MyCnf | Dialect::Postgres => '#',
    };
    let comment_out = |line: &mut String| {
        let indent_len = line.len() - line.trim_start().len();
        line.insert(indent_len, comment_char);
    };

    match value {
        None => {
            for i in active {
                comment_out(&mut lines[i]);
            }
        }
        Some(v) => {
            let new_line = dialect.format(key, v);
            if let Some((&last, earlier)) = active.split_last() {
                for &i in earlier {
                    comment_out(&mut lines[i]);
                }
                let t = terminator(&lines[last]);
                lines[last] = format!("{new_line}{t}");
            } else if let Some(&last) = commented.last() {
                let t = terminator(&lines[last]);
                lines[last] = format!("{new_line}{t}");
            } else {
                let at = match dialect {
                    Dialect::MyCnf if seen_server_section => {
                        insert_at.unwrap_or_else(|| last_content_line(&lines, lines.len()) + 1)
                    }
                    Dialect::MyCnf => {
                        // No server section: add one at the end.
                        ensure_trailing_newline(&mut lines, nl);
                        lines.push(format!("[mysqld]{nl}"));
                        lines.len()
                    }
                    Dialect::Php => insert_at.unwrap_or(lines.len()),
                    Dialect::Postgres => lines.len(),
                };
                if at > 0 && at <= lines.len() {
                    let prev = &mut lines[at - 1];
                    if terminator(prev).is_empty() {
                        prev.push_str(nl);
                    }
                }
                lines.insert(at, format!("{new_line}{nl}"));
            }
        }
    }
    lines.concat()
}

/// Index of the last non-blank line before `end` (so additions sit right after a section's
/// content, not after the blank lines separating it from the next one).
fn last_content_line(lines: &[String], end: usize) -> usize {
    (0..end)
        .rev()
        .find(|&i| !lines[i].trim().is_empty())
        .unwrap_or(0)
}

fn ensure_trailing_newline(lines: &mut [String], nl: &str) {
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push_str(nl);
    }
}

// ---- php.ini extensions -------------------------------------------------------------------

/// Extensions PHP loads with `zend_extension=` instead of `extension=`.
fn is_zend_extension(name: &str) -> bool {
    matches!(name, "opcache" | "xdebug")
}

/// `php_curl.dll` / `"curl"` / `curl` → `curl`.
fn extension_name(value: &str) -> String {
    let v = unquote(value).to_ascii_lowercase();
    let file = v.rsplit(['/', '\\']).next().unwrap_or(&v);
    let file = file.strip_suffix(".dll").unwrap_or(file);
    file.strip_prefix("php_").unwrap_or(file).to_owned()
}

/// Extensions php.ini enables (`extension=` and `zend_extension=` lines), lower-case names.
pub fn php_extensions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = ["extension", "zend_extension"]
        .iter()
        .flat_map(|k| get_all(text, Dialect::Php, k))
        .map(|v| extension_name(&v))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Turns one extension on or off. Enabling uncomments the template's `;extension=name` line
/// when there is one; disabling comments out every line that loads it.
pub fn set_php_extension(text: &str, name: &str, enabled: bool) -> String {
    let name = name.to_ascii_lowercase();
    let key = if is_zend_extension(&name) {
        "zend_extension"
    } else {
        "extension"
    };
    let nl = newline_of(text);
    let mut lines: Vec<String> = split_lines(text).into_iter().map(str::to_owned).collect();
    let mut active = Vec::new();
    let mut commented = Vec::new();
    let mut first_scoped: Option<usize> = None;
    let mut section_scoped = false;
    for (i, line) in lines.iter().enumerate() {
        if let Some(s) = section_name(line) {
            section_scoped = is_php_scoped_section(s);
            if section_scoped && first_scoped.is_none() {
                first_scoped = Some(i);
            }
            continue;
        }
        if section_scoped {
            continue;
        }
        if let Some(e) = parse_line(Dialect::Php, line)
            && (e.key == "extension" || e.key == "zend_extension")
            && extension_name(&e.value) == name
        {
            if e.commented {
                commented.push(i);
            } else {
                active.push(i);
            }
        }
    }
    if enabled {
        if !active.is_empty() {
            return text.to_owned();
        }
        let line = format!("{key}={name}");
        // The last one: php.ini's explanatory comments come before the template's list.
        if let Some(&i) = commented.last() {
            let t = if lines[i].ends_with("\r\n") {
                "\r\n"
            } else if lines[i].ends_with('\n') {
                "\n"
            } else {
                ""
            };
            lines[i] = format!("{line}{t}");
        } else {
            let at = first_scoped.unwrap_or(lines.len());
            if at > 0 && !lines[at - 1].ends_with('\n') {
                lines[at - 1].push_str(nl);
            }
            lines.insert(at, format!("{line}{nl}"));
        }
    } else {
        for i in active {
            let indent = lines[i].len() - lines[i].trim_start().len();
            lines[i].insert(indent, ';');
        }
    }
    lines.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHP: &str = "[PHP]\r\n; Maximum amount of memory\r\nmemory_limit = 128M\r\n\r\n;upload_max_filesize = 2M\r\n[Date]\r\n;date.timezone =\r\n;extension=curl\r\n;extension=mbstring\r\nextension=php_openssl.dll\r\n";

    #[test]
    fn php_values_are_replaced_in_place_and_line_endings_kept() {
        assert_eq!(
            get(PHP, Dialect::Php, "memory_limit").as_deref(),
            Some("128M")
        );
        assert_eq!(get(PHP, Dialect::Php, "upload_max_filesize"), None);

        let out = set(PHP, Dialect::Php, "memory_limit", Some("512M"));
        assert_eq!(
            out,
            PHP.replace("memory_limit = 128M", "memory_limit = 512M")
        );

        // Uncomments the template's own line instead of adding a new one.
        let out = set(PHP, Dialect::Php, "date.timezone", Some("\"Asia/Karachi\""));
        assert!(out.contains("[Date]\r\ndate.timezone = \"Asia/Karachi\"\r\n"));
        assert_eq!(out.lines().count(), PHP.lines().count());

        // Unset = comment out, so PHP's default applies again.
        let out = set(PHP, Dialect::Php, "memory_limit", None);
        assert!(out.contains(";memory_limit = 128M\r\n"));
        assert_eq!(get(&out, Dialect::Php, "memory_limit"), None);
    }

    #[test]
    fn php_last_line_wins_and_earlier_duplicates_are_commented() {
        let text = "a = 1\nb = 2\na = 3\n";
        assert_eq!(get(text, Dialect::Php, "a").as_deref(), Some("3"));
        let out = set(text, Dialect::Php, "a", Some("9"));
        assert_eq!(out, ";a = 1\nb = 2\na = 9\n");
    }

    #[test]
    fn php_new_keys_go_before_scoped_sections() {
        let text = "x = 1\n[PATH=C:/site]\ndisplay_errors = On";
        assert_eq!(
            get(text, Dialect::Php, "display_errors"),
            None,
            "scoped to one folder"
        );
        let out = set(text, Dialect::Php, "display_errors", Some("Off"));
        assert_eq!(
            out,
            "x = 1\ndisplay_errors = Off\n[PATH=C:/site]\ndisplay_errors = On"
        );
        // A file without a trailing newline gets one before the added line.
        assert_eq!(set("x = 1", Dialect::Php, "y", Some("2")), "x = 1\ny = 2\n");
    }

    #[test]
    fn php_extensions_toggle_template_lines() {
        assert_eq!(php_extensions(PHP), vec!["openssl"]);
        let on = set_php_extension(PHP, "curl", true);
        assert!(on.contains("\r\nextension=curl\r\n"));
        assert_eq!(php_extensions(&on), vec!["curl", "openssl"]);
        assert_eq!(on.lines().count(), PHP.lines().count());

        let off = set_php_extension(&on, "openssl", false);
        assert!(off.contains(";extension=php_openssl.dll"));
        assert_eq!(php_extensions(&off), vec!["curl"]);

        let opcache = set_php_extension("a = 1\n", "opcache", true);
        assert_eq!(opcache, "a = 1\nzend_extension=opcache\n");
        assert_eq!(
            set_php_extension(&on, "curl", true),
            on,
            "already on: unchanged"
        );
    }

    const MY_INI: &str =
        "[mysqld]\ndatadir=C:/data\nport=3306\n\n[client]\nport=3306\nmax_allowed_packet=1M\n";

    #[test]
    fn my_ini_edits_stay_in_the_server_section() {
        assert_eq!(
            get(MY_INI, Dialect::MyCnf, "max_allowed_packet"),
            None,
            "[client] only"
        );
        let out = set(MY_INI, Dialect::MyCnf, "max-allowed-packet", Some("64M"));
        assert_eq!(
            out,
            "[mysqld]\ndatadir=C:/data\nport=3306\nmax-allowed-packet=64M\n\n[client]\nport=3306\nmax_allowed_packet=1M\n"
        );
        assert_eq!(
            get(&out, Dialect::MyCnf, "max_allowed_packet").as_deref(),
            Some("64M")
        );
        let again = set(&out, Dialect::MyCnf, "max_allowed_packet", Some("128M"));
        assert!(again.contains("[mysqld]\ndatadir=C:/data\nport=3306\nmax_allowed_packet=128M\n"));

        let none = set(
            "[client]\nport=1\n",
            Dialect::MyCnf,
            "max_connections",
            Some("50"),
        );
        assert_eq!(none, "[client]\nport=1\n[mysqld]\nmax_connections=50\n");
        let tail = set(
            "[mysqld]\nport=1",
            Dialect::MyCnf,
            "max_connections",
            Some("50"),
        );
        assert_eq!(tail, "[mysqld]\nport=1\nmax_connections=50\n");
    }

    #[test]
    fn postgres_uncomments_initdb_defaults_and_ignores_trailing_comments() {
        let text = "max_connections = 100\t\t# (change requires restart)\n#shared_buffers = 128MB\t\t# min 128kB\nlog_statement = 'none # not a comment'\n";
        assert_eq!(
            get(text, Dialect::Postgres, "max_connections").as_deref(),
            Some("100")
        );
        assert_eq!(
            get(text, Dialect::Postgres, "log_statement").as_deref(),
            Some("'none # not a comment'")
        );
        assert_eq!(get(text, Dialect::Postgres, "shared_buffers"), None);
        let out = set(text, Dialect::Postgres, "shared_buffers", Some("256MB"));
        assert!(out.contains("\nshared_buffers = 256MB\n"));
        assert_eq!(out.lines().count(), 3);
    }
}
