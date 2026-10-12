//! Path containment checks (ADR 0005). Use before any write or delete under an app root.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathError {
    #[error("path escapes its allowed root")]
    Escapes,
    #[error("path is not usable: {0}")]
    Invalid(String),
}

/// Joins a *relative* path onto `root`, rejecting absolute paths, drive/UNC prefixes,
/// `..`, and Windows-hostile names (reserved devices, alternate data streams).
pub fn join_relative(root: &Path, relative: &str) -> Result<PathBuf, PathError> {
    let normalized = relative.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') {
        return Err(PathError::Escapes);
    }
    let mut out = root.to_path_buf();
    for seg in normalized.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return Err(PathError::Escapes),
            _ => {}
        }
        if seg.contains(':') {
            return Err(PathError::Invalid(format!("\"{seg}\" contains ':'")));
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(PathError::Invalid(format!(
                "\"{seg}\" ends with a dot or space"
            )));
        }
        if seg
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err(PathError::Invalid(format!(
                "\"{seg}\" contains invalid characters"
            )));
        }
        if is_reserved_device_name(seg) {
            return Err(PathError::Invalid(format!(
                "\"{seg}\" is a reserved device name"
            )));
        }
        out.push(seg);
    }
    // Belt and braces: the component walk must stay below root.
    if out.components().any(|c| matches!(c, Component::ParentDir)) || !out.starts_with(root) {
        return Err(PathError::Escapes);
    }
    Ok(out)
}

fn is_reserved_device_name(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

/// Verifies that an *existing* path resolves (following links/junctions) inside `root`.
pub fn ensure_within(root: &Path, path: &Path) -> Result<PathBuf, PathError> {
    let root = dunce_canonical(root)?;
    let path = dunce_canonical(path)?;
    if path.starts_with(&root) {
        Ok(path)
    } else {
        Err(PathError::Escapes)
    }
}

fn dunce_canonical(p: &Path) -> Result<PathBuf, PathError> {
    std::fs::canonicalize(p).map_err(|e| PathError::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_normal_relative_paths() {
        let root = Path::new("C:\\root");
        assert_eq!(
            join_relative(root, "bin/php.exe").unwrap(),
            root.join("bin").join("php.exe")
        );
        assert_eq!(
            join_relative(root, "ext\\php_curl.dll").unwrap(),
            root.join("ext").join("php_curl.dll")
        );
    }

    #[test]
    fn rejects_escapes_and_hostile_names() {
        let root = Path::new("C:\\root");
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "\\Windows\\x",
            "C:/Windows/x",
            "file.txt:stream",
            "NUL",
            "com1.txt",
            "dir./x",
            "x ",
            "a/b?c",
            "",
        ] {
            assert!(
                join_relative(root, bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(
            join_relative(root, "console.log").is_ok(),
            "only exact device names are reserved"
        );
    }

    #[test]
    fn ensure_within_follows_real_locations() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("a");
        std::fs::create_dir(&inner).unwrap();
        assert!(ensure_within(dir.path(), &inner).is_ok());
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            ensure_within(dir.path(), other.path()),
            Err(PathError::Escapes)
        );
    }

    /// Cross-cutting: a directory junction inside the root that points outside it is an
    /// escape (junctions need no admin rights, so they're the realistic indirection).
    #[test]
    fn junctions_pointing_outside_are_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        let system = std::env::var("SystemRoot").unwrap();
        let out = std::process::Command::new(format!(r"{system}\System32\cmd.exe"))
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(ensure_within(dir.path(), &link), Err(PathError::Escapes));
        assert_eq!(
            ensure_within(dir.path(), &link.join(".")),
            Err(PathError::Escapes)
        );
        // A junction that stays inside is fine.
        let inner = dir.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        let ok_link = dir.path().join("ok");
        let out = std::process::Command::new(format!(r"{system}\System32\cmd.exe"))
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&ok_link)
            .arg(&inner)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(ensure_within(dir.path(), &ok_link).is_ok());
    }
}
