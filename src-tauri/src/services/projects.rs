//! Project registration. Projects live in `<root>\www\<folder>`; Devluma creates the folder
//! when it doesn't exist yet but never writes into it, and removing a project never deletes
//! its files.

use std::path::{Path, PathBuf};

use crate::error::{ErrorCode, IpcError};
use crate::persistence::Database;
use crate::persistence::projects::{self, Project, ProjectFields};
use crate::state::DataPaths;

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(ErrorCode::ValidationError, msg, false)
}

/// `\\?\C:\x` → `C:\x` for display and storage (canonicalize adds the verbatim prefix).
fn simplify(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// Resolves a folder relative to `www\` (for example `shop` or `clients\shop`). Absolute
/// paths, `..` and Windows-hostile names are rejected.
fn project_dir(folder: &str, paths: &DataPaths) -> Result<PathBuf, IpcError> {
    let folder = folder.trim();
    let example = "Enter a folder inside www, for example my-app.";
    let dir = crate::security::paths::join_relative(&paths.www, folder).map_err(|e| match e {
        crate::security::paths::PathError::Invalid(why) => invalid(format!("{example} ({why})")),
        crate::security::paths::PathError::Escapes => invalid(example),
    })?;
    if dir == paths.www {
        return Err(invalid(example));
    }
    Ok(dir)
}

/// Registers `www\<folder>` as a project, creating the folder if it doesn't exist yet. An
/// existing folder (for example a cloned repository) is used as it is.
pub fn create(
    db: &Database,
    paths: &DataPaths,
    name: &str,
    folder: &str,
) -> Result<Project, IpcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("Enter a project name."));
    }
    let dir = project_dir(folder, paths)?;
    if dir.exists() && !dir.is_dir() {
        return Err(invalid("That path is a file, not a folder."));
    }
    // Folders this call creates, deepest first, so a failure removes them again (only while
    // they are still empty).
    let created: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|a| *a != paths.www && !a.exists())
        .map(Path::to_path_buf)
        .collect();
    let undo = || {
        for d in &created {
            let _ = std::fs::remove_dir(d);
        }
    };
    std::fs::create_dir_all(&dir)
        .map_err(|_| invalid("Devluma couldn't create that folder inside www."))?;
    // A junction inside www must not lead a project elsewhere.
    let canonical = match crate::security::paths::ensure_within(&paths.www, &dir) {
        Ok(c) => c,
        Err(_) => {
            undo();
            return Err(invalid("That folder leads outside www."));
        }
    };
    let root = simplify(&canonical).display().to_string();
    db.write(|c| {
        if projects::list(c)?
            .iter()
            .any(|p| p.root_path.eq_ignore_ascii_case(&root))
        {
            return Err(crate::persistence::DbError::Invariant(
                "That folder is already added as a project.".into(),
            ));
        }
        projects::create(
            c,
            &ProjectFields {
                name: name.to_owned(),
                root_path: root.clone(),
                project_type: detect(Path::new(&root)).kind.map(str::to_owned),
                ..Default::default()
            },
        )
    })
    .map_err(|e| {
        undo();
        IpcError::from(e)
    })
}

/// What a project folder looks like, judged only by which files exist (app_flow.md §6:
/// detection never runs or parses project code).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detection {
    pub kind: Option<&'static str>,
    pub label: &'static str,
    /// Folder a web server should serve.
    pub suggested_document_root: String,
    /// `Scripts` folder of a Python virtual environment in the project (`.venv` or `venv`),
    /// put first on PATH in project terminals.
    pub virtual_env_scripts: Option<String>,
}

/// A Python virtual environment inside the project, found by its layout only.
fn virtual_env_scripts(root: &Path) -> Option<PathBuf> {
    [".venv", "venv"].iter().find_map(|name| {
        let scripts = root.join(name).join("Scripts");
        (scripts.join("python.exe").is_file() && root.join(name).join("pyvenv.cfg").is_file())
            .then_some(scripts)
    })
}

pub fn detect(root: &Path) -> Detection {
    let has = |rel: &str| root.join(rel).exists();
    let public = root.join("public");
    let (kind, label, docroot): (Option<&'static str>, &'static str, PathBuf) =
        if has("artisan") && has("public/index.php") {
            (Some("laravel"), "Laravel", public)
        } else if has("bin/console") && has("public/index.php") {
            (Some("symfony"), "Symfony", public)
        } else if has("wp-config.php") || has("wp-load.php") {
            (Some("wordpress"), "WordPress", root.to_path_buf())
        } else if has("composer.json") || has("index.php") || has("public/index.php") {
            let docroot = if has("public/index.php") {
                public
            } else {
                root.to_path_buf()
            };
            (Some("php"), "PHP", docroot)
        } else if has("package.json") {
            (Some("node"), "Node.js", root.to_path_buf())
        } else if has("pyproject.toml") || has("requirements.txt") || has("manage.py") {
            (Some("python"), "Python", root.to_path_buf())
        } else if has("index.html") {
            (Some("static"), "Static site", root.to_path_buf())
        } else {
            (None, "Unknown", root.to_path_buf())
        };
    Detection {
        kind,
        label,
        suggested_document_root: docroot.display().to_string(),
        virtual_env_scripts: virtual_env_scripts(root).map(|p| p.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    #[test]
    fn detects_project_types_from_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let touch = |rel: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        };
        assert_eq!(detect(dir.path()).kind, None);
        touch("index.html");
        assert_eq!(detect(dir.path()).kind, Some("static"));
        touch("composer.json");
        assert_eq!(detect(dir.path()).kind, Some("php"));
        touch("public/index.php");
        touch("artisan");
        let d = detect(dir.path());
        assert_eq!((d.kind, d.label), (Some("laravel"), "Laravel"));
        assert!(d.suggested_document_root.ends_with("public"));
        assert_eq!(d.virtual_env_scripts, None);
    }

    #[test]
    fn finds_python_virtual_environments_by_layout() {
        let dir = tempfile::tempdir().unwrap();
        let touch = |rel: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        };
        touch("pyproject.toml");
        touch(".venv/Scripts/python.exe");
        assert_eq!(
            detect(dir.path()).virtual_env_scripts,
            None,
            "needs pyvenv.cfg too"
        );
        touch(".venv/pyvenv.cfg");
        let d = detect(dir.path());
        assert_eq!(d.kind, Some("python"));
        assert!(d.virtual_env_scripts.unwrap().ends_with(r".venv\Scripts"));
    }

    fn paths() -> (tempfile::TempDir, DataPaths) {
        let dir = tempfile::tempdir().unwrap();
        let p = DataPaths::under(&dir.path().join("Devluma"));
        p.create_all().unwrap();
        (dir, p)
    }

    #[test]
    fn creates_projects_inside_www_only() {
        let (dir, paths) = paths();
        let db = db();

        // A missing folder is created.
        let p = create(&db, &paths, " Shop ", "shop").unwrap();
        assert_eq!(p.name, "Shop");
        assert!(paths.www.join("shop").is_dir());
        assert!(!p.root_path.starts_with(r"\\?\"), "{}", p.root_path);
        assert!(p.root_path.ends_with(r"www\shop"), "{}", p.root_path);

        // An existing folder is used as it is, and its type detected.
        std::fs::create_dir_all(paths.www.join(r"clients\blog")).unwrap();
        std::fs::write(paths.www.join(r"clients\blog\index.html"), "").unwrap();
        let blog = create(&db, &paths, "Blog", "clients/blog").unwrap();
        assert_eq!(blog.project_type.as_deref(), Some("static"));

        assert!(
            create(&db, &paths, "Again", "SHOP")
                .unwrap_err()
                .message
                .contains("already added")
        );
        for bad in [
            "",
            ".",
            r"..\escape",
            r"shop\..\..\escape",
            &dir.path().join("abs").to_string_lossy(),
            r"C:\code\x",
            "con",
            "a:b",
        ] {
            assert!(create(&db, &paths, "x", bad).is_err(), "{bad}");
        }
        assert!(!dir.path().join("escape").exists());
        assert!(!dir.path().join("abs").exists());
        std::fs::write(paths.www.join("file.txt"), "").unwrap();
        assert!(create(&db, &paths, "x", "file.txt").is_err());
        assert!(create(&db, &paths, "  ", "other").is_err());
        assert!(!paths.www.join("other").exists());
    }

    #[test]
    fn a_failed_create_removes_only_the_folders_it_made() {
        let (_dir, paths) = paths();
        let db = db();
        create(&db, &paths, "Shop", "shop").unwrap();
        std::fs::write(paths.www.join(r"shop\keep.txt"), "").unwrap();
        // The same folder written differently: rejected as a duplicate, folder kept.
        assert!(create(&db, &paths, "Shop", r"shop\.").is_err());
        assert!(paths.www.join(r"shop\keep.txt").is_file());

        // A junction inside www that leads elsewhere is refused and the new folders removed.
        let outside = tempfile::tempdir().unwrap();
        let link = paths.www.join("linked");
        let system = std::env::var("SystemRoot").unwrap();
        let out = std::process::Command::new(format!(r"{system}\System32\cmd.exe"))
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        let err = create(&db, &paths, "x", r"linked\new\deep").unwrap_err();
        assert!(err.message.contains("outside www"), "{}", err.message);
        assert!(!outside.path().join("new").exists());
    }
}
