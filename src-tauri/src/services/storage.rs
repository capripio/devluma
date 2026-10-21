//! Where Devluma keeps everything (ADR 0014, supersedes ADR 0005's fixed location).
//!
//! Release builds are portable by default: the storage root is the folder that contains
//! `devluma.exe`, so the app, its downloaded tools (`bin\`), data, configs, logs, backups and
//! temp files all live together and the folder can be moved or copied. A `devluma-root.txt`
//! next to the exe (written by the first-run wizard) points somewhere else instead. If the
//! exe's folder isn't writable (e.g. Program Files) the per-user folder
//! `%LOCALAPPDATA%\Devluma` is used. Debug builds use `DEVLUMA_ROOT` or the per-user folder,
//! so development data never lands in `target\`.
//!
//! When the root changes (the folder was moved, or the old `packages\` layout is found),
//! `relocate` rewrites the stored absolute paths and the few Devluma-written files that
//! contain them, and journals what changed. User files outside the root are never touched.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::persistence::operations::{EventLevel, NewOperation};
use crate::persistence::{Database, DbResult, settings};

pub const POINTER_FILE: &str = "devluma-root.txt";
const ROOT_SETTING: &str = "storage.root";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageMode {
    /// Next to devluma.exe.
    Portable,
    /// A folder chosen in the wizard (`devluma-root.txt`).
    Custom,
    /// `%LOCALAPPDATA%\Devluma` (debug builds, or the exe's folder isn't writable).
    Profile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageChoice {
    pub root: PathBuf,
    pub mode: StorageMode,
    pub exe_dir: Option<PathBuf>,
    pub exe_dir_writable: bool,
}

pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Can Devluma create files in `dir`? (Program Files can't be written without admin.)
pub fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".devluma-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn profile_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Devluma")
}

/// Reads `devluma-root.txt` (first non-empty line; relative paths are relative to the exe).
pub fn read_pointer(exe_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(exe_dir.join(POINTER_FILE)).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let p = PathBuf::from(line);
    Some(if p.is_absolute() { p } else { exe_dir.join(p) })
}

/// The storage root for this run (see the module docs for the rules).
pub fn resolve(debug_build: bool) -> StorageChoice {
    let exe = exe_dir();
    let writable = exe.as_deref().is_some_and(is_writable);
    if debug_build {
        let root = std::env::var_os("DEVLUMA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(profile_root);
        return StorageChoice {
            root,
            mode: StorageMode::Profile,
            exe_dir: exe,
            exe_dir_writable: writable,
        };
    }
    resolve_for(exe, writable)
}

/// Release-build rules, separated for tests.
pub fn resolve_for(exe: Option<PathBuf>, writable: bool) -> StorageChoice {
    if let Some(dir) = &exe {
        if let Some(root) = read_pointer(dir) {
            return StorageChoice {
                root,
                mode: StorageMode::Custom,
                exe_dir: exe,
                exe_dir_writable: writable,
            };
        }
        if writable {
            return StorageChoice {
                root: dir.clone(),
                mode: StorageMode::Portable,
                exe_dir: exe,
                exe_dir_writable: writable,
            };
        }
    }
    StorageChoice {
        root: profile_root(),
        mode: StorageMode::Profile,
        exe_dir: exe,
        exe_dir_writable: writable,
    }
}

/// Points this installation at `root` from the next start (`None`: back to the exe's folder).
pub fn write_pointer(exe_dir: &Path, root: Option<&Path>) -> std::io::Result<()> {
    let file = exe_dir.join(POINTER_FILE);
    match root {
        Some(r) => std::fs::write(
            file,
            format!(
                "{}\n# Devluma keeps its tools, data and settings in the folder above. Delete this file to use the folder that contains devluma.exe.\n",
                r.display()
            ),
        ),
        None if file.exists() => std::fs::remove_file(file),
        None => Ok(()),
    }
}

// ---- relocation ------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelocationReport {
    pub previous_root: Option<String>,
    pub renamed_packages_folder: bool,
    pub rows_updated: usize,
    pub files_updated: usize,
}

/// Replaces `old` with `new` where `old` is a whole path prefix (followed by a separator,
/// quote, whitespace or the end). Case-insensitive (Windows paths), both separator styles.
pub fn replace_path_prefix(text: &str, old: &str, new: &str) -> String {
    let mut out = text.to_owned();
    for (o, n) in [
        (old.to_owned(), new.to_owned()),
        (old.replace('\\', "/"), new.replace('\\', "/")),
    ] {
        if o.is_empty() || !o.is_ascii() {
            continue;
        }
        let lower = out.to_ascii_lowercase();
        let needle = o.to_ascii_lowercase();
        let mut result = String::with_capacity(out.len());
        let mut i = 0;
        while let Some(pos) = lower[i..].find(&needle) {
            let start = i + pos;
            let end = start + o.len();
            let next = out[end..].chars().next();
            let boundary = matches!(
                next,
                None | Some('\\' | '/' | '"' | '\'' | ' ' | '\r' | '\n' | ';' | ')')
            );
            result.push_str(&out[i..start]);
            result.push_str(if boundary { &n } else { &out[start..end] });
            i = end;
        }
        result.push_str(&out[i..]);
        out = result;
    }
    out
}

fn rewrite_columns(c: &rusqlite::Connection, maps: &[(String, String)]) -> DbResult<usize> {
    const COLUMNS: &[(&str, &str, &str)] = &[
        ("package_installations", "id", "install_path"),
        ("service_instances", "id", "config_path"),
        ("service_instances", "id", "data_path"),
        ("sites", "id", "config_path"),
        ("sites", "id", "document_root"),
        ("projects", "id", "root_path"),
        ("certificates", "id", "certificate_path"),
        ("certificates", "id", "private_key_path"),
        ("database_backups", "id", "path"),
        ("hosts_backups", "id", "backup_path"),
        // A scheduled command may name an .exe inside Devluma's folder (ADR 0017).
        ("scheduled_jobs", "id", "program"),
    ];
    let mut n = 0;
    for (table, key, col) in COLUMNS {
        let rows: Vec<(String, String)> = {
            let mut stmt = c.prepare(&format!(
                "SELECT {key}, {col} FROM {table} WHERE {col} IS NOT NULL"
            ))?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        for (id, value) in rows {
            let new = maps
                .iter()
                .fold(value.clone(), |v, (o, n)| replace_path_prefix(&v, o, n));
            if new != value {
                c.execute(
                    &format!("UPDATE {table} SET {col} = ?2 WHERE {key} = ?1"),
                    rusqlite::params![id, new],
                )?;
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Devluma-written files that contain absolute paths: site configs and php.ini files.
fn managed_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let dirs = |p: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()
    };
    for svc in dirs(&root.join("services")) {
        for f in std::fs::read_dir(svc.join("sites"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if f.path().extension().is_some_and(|e| e == "conf") {
                files.push(f.path());
            }
        }
    }
    for ver in dirs(&root.join("bin").join("php")) {
        let ini = ver.join("php.ini");
        if ini.is_file() {
            files.push(ini);
        }
    }
    files
}

/// Brings stored paths in line with the current root. Runs at startup, before anything
/// reads them. Idempotent: nothing changes when the root and layout are already current.
pub fn relocate(db: &Database, root: &Path) -> DbResult<RelocationReport> {
    let mut report = RelocationReport::default();
    let root_s = root.display().to_string();
    let previous: Option<String> = db.read(|c| settings::get::<String>(c, ROOT_SETTING))?;

    // Old layout: packages\ → bin\ (Phase 0–10 builds).
    let (old_pkgs, bin) = (root.join("packages"), root.join("bin"));
    let bin_empty = std::fs::read_dir(&bin)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if old_pkgs.is_dir() && bin_empty {
        if bin.exists() {
            let _ = std::fs::remove_dir(&bin); // created empty by startup's create_all
        }
        std::fs::rename(&old_pkgs, &bin).map_err(|e| crate::persistence::DbError::Invariant(format!(
            "Could not rename {} to {}: {e}. Close programs using files in it and restart Devluma.",
            old_pkgs.display(),
            bin.display()
        )))?;
        report.renamed_packages_folder = true;
    }

    let mut maps: Vec<(String, String)> = Vec::new();
    if let Some(prev) = previous
        .as_deref()
        .filter(|p| !p.eq_ignore_ascii_case(&root_s))
    {
        maps.push((format!("{prev}\\packages"), format!("{root_s}\\bin")));
        maps.push((prev.to_owned(), root_s.clone()));
        report.previous_root = Some(prev.to_owned());
    }
    // Rows written by builds that used packages\ (also covers an unchanged root).
    maps.push((format!("{root_s}\\packages"), format!("{root_s}\\bin")));

    report.rows_updated = db.write(|c| rewrite_columns(c, &maps))?;
    for file in managed_files(root) {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let new = maps
                .iter()
                .fold(text.clone(), |t, (o, n)| replace_path_prefix(&t, o, n));
            if new != text && std::fs::write(&file, new).is_ok() {
                report.files_updated += 1;
            }
        }
    }
    db.write(|c| settings::set(c, ROOT_SETTING, &root_s))?;

    if report.previous_root.is_some() || report.renamed_packages_folder || report.rows_updated > 0 {
        let sink = crate::services::journal::NullSink;
        let journal = crate::services::journal::Journal::new(db, &sink);
        if let Ok(op) = journal.start(&NewOperation {
            operation_type: "relocate_storage",
            target_type: None,
            target_id: None,
        }) {
            let _ = journal.running(&op.id);
            let _ = journal.event(
                &op.id,
                EventLevel::Info,
                &format!(
                    "Storage folder is {root_s}{}. Updated {} stored paths and {} files.",
                    report
                        .previous_root
                        .as_deref()
                        .map_or(String::new(), |p| format!(" (was {p})")),
                    report.rows_updated,
                    report.files_updated
                ),
                &serde_json::to_value(&report).unwrap_or_default(),
            );
            let _ = journal.succeed(&op.id);
        }
    }
    Ok(report)
}

/// Size of one of Devluma's folders, for Settings → Storage.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderUsage {
    pub key: &'static str,
    pub path: String,
    pub bytes: u64,
    pub files: u64,
}

/// Devluma's folders under the storage root, by key.
pub fn folders(paths: &crate::state::DataPaths) -> [(&'static str, PathBuf); 7] {
    [
        ("www", paths.www.clone()),
        ("packages", paths.packages.clone()),
        ("data", paths.data.clone()),
        ("backups", paths.backups.clone()),
        ("services", paths.services.clone()),
        ("staging", paths.staging.clone()),
        ("temp", paths.root.join("tmp")),
    ]
}

/// Sizes of `folders`. Junctions and symlinks are never followed.
pub fn usage(paths: &crate::state::DataPaths) -> Vec<FolderUsage> {
    folders(paths)
        .into_iter()
        .map(|(key, path)| {
            let (bytes, files) = folder_size(&path);
            FolderUsage {
                key,
                path: path.display().to_string(),
                bytes,
                files,
            }
        })
        .collect()
}

fn folder_size(dir: &Path) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() && !is_reparse_point(&meta) {
                stack.push(entry.path());
            } else if meta.is_file() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    #[test]
    fn usage_counts_files_and_skips_missing_folders() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::state::DataPaths::under(dir.path());
        std::fs::create_dir_all(paths.packages.join("php").join("8.5")).unwrap();
        std::fs::write(
            paths.packages.join("php").join("8.5").join("php.exe"),
            [0u8; 100],
        )
        .unwrap();
        std::fs::write(paths.packages.join("a.txt"), [0u8; 20]).unwrap();
        let u = usage(&paths);
        let packages = u.iter().find(|f| f.key == "packages").unwrap();
        assert_eq!((packages.bytes, packages.files), (120, 2));
        let data = u.iter().find(|f| f.key == "data").unwrap();
        assert_eq!((data.bytes, data.files), (0, 0));
    }

    #[test]
    fn prefixes_are_replaced_only_at_path_boundaries() {
        let t = r#"root "C:/Old/Devluma/bin/php"; x C:\OLD\Devluma\data and C:\Old\DevlumaX\keep"#;
        let r = replace_path_prefix(t, r"C:\Old\Devluma", r"D:\New");
        assert_eq!(
            r,
            r#"root "D:/New/bin/php"; x D:\New\data and C:\Old\DevlumaX\keep"#
        );
    }

    #[test]
    fn release_rules_pick_portable_pointer_or_profile() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("app");
        std::fs::create_dir(&exe).unwrap();
        let c = resolve_for(Some(exe.clone()), true);
        assert_eq!(
            (c.mode, c.root.as_path()),
            (StorageMode::Portable, exe.as_path())
        );
        assert_eq!(
            resolve_for(Some(exe.clone()), false).mode,
            StorageMode::Profile
        );
        write_pointer(&exe, Some(Path::new(r"D:\DevData"))).unwrap();
        let c = resolve_for(Some(exe.clone()), true);
        assert_eq!(
            (c.mode, c.root),
            (StorageMode::Custom, PathBuf::from(r"D:\DevData"))
        );
        std::fs::write(exe.join(POINTER_FILE), "data\n").unwrap();
        assert_eq!(
            resolve_for(Some(exe.clone()), true).root,
            exe.join("data"),
            "relative to the exe"
        );
        write_pointer(&exe, None).unwrap();
        assert_eq!(resolve_for(Some(exe), true).mode, StorageMode::Portable);
    }

    #[test]
    fn moving_the_folder_rewrites_stored_paths_and_managed_files() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("moved"));
        let db = db();
        db.write(|c| settings::set(c, ROOT_SETTING, &old.display().to_string()))
            .unwrap();
        // An old-layout install row and an outside project.
        let old_php = old.join("packages").join("php").join("8.4.26-x64");
        db.write(|c| {
            c.execute_batch("PRAGMA defer_foreign_keys = ON")?;
            c.execute(
                "INSERT INTO projects (id, name, root_path, is_favorite, created_at, updated_at) VALUES ('p1', 'Out', 'D:\\code\\shop', 0, 'x', 'x')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        // The folder has moved (and still has the old packages\ layout).
        let php_dir = new.join("packages").join("php").join("8.4.26-x64");
        std::fs::create_dir_all(&php_dir).unwrap();
        std::fs::write(
            php_dir.join("php.ini"),
            format!("extension_dir = \"{}\\ext\"\n", old_php.display()),
        )
        .unwrap();
        let site_dir = new.join("services").join("w1").join("sites");
        std::fs::create_dir_all(&site_dir).unwrap();
        let old_fwd = old.display().to_string().replace('\\', "/");
        std::fs::write(
            site_dir.join("s1.conf"),
            format!("include \"{old_fwd}/packages/nginx/1.30.5-x64/conf/fastcgi_params\";\n"),
        )
        .unwrap();

        let report = relocate(&db, &new).unwrap();
        assert!(report.renamed_packages_folder);
        assert_eq!(report.files_updated, 2);
        let ini = std::fs::read_to_string(
            new.join("bin")
                .join("php")
                .join("8.4.26-x64")
                .join("php.ini"),
        )
        .unwrap();
        assert!(
            ini.contains(&format!(
                "{}\\ext",
                new.join("bin").join("php").join("8.4.26-x64").display()
            )),
            "{ini}"
        );
        let conf = std::fs::read_to_string(site_dir.join("s1.conf")).unwrap();
        assert!(
            conf.contains(&format!(
                "{}/bin/nginx",
                new.display().to_string().replace('\\', "/")
            )),
            "{conf}"
        );
        let project: String = db
            .read(|c| Ok(c.query_row("SELECT root_path FROM projects", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(
            project, r"D:\code\shop",
            "paths outside the root are untouched"
        );
        // Second start: nothing to do.
        assert_eq!(relocate(&db, &new).unwrap(), RelocationReport::default());
    }
}
