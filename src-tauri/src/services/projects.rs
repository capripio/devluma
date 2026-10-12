//! Project registration. Devluma only records metadata about a folder the user chooses; it
//! never writes into project folders, and removing a project never deletes its files.

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

/// Validates a user-entered project folder and returns its canonical form.
pub fn validate_root(input: &str, paths: &DataPaths) -> Result<String, IpcError> {
    let raw = Path::new(input.trim());
    if !raw.is_absolute() {
        return Err(invalid(
            "Enter the full folder path, for example D:\\code\\my-app.",
        ));
    }
    let canonical = std::fs::canonicalize(raw)
        .map_err(|_| invalid("That folder doesn't exist or can't be opened."))?;
    if !canonical.is_dir() {
        return Err(invalid("That path is a file, not a folder."));
    }
    // Projects must not live inside Devluma's own managed folders.
    if let Ok(root) = std::fs::canonicalize(&paths.root)
        && canonical.starts_with(&root)
    {
        return Err(invalid("Choose a folder outside Devluma's data folder."));
    }
    Ok(simplify(&canonical).display().to_string())
}

pub fn create(
    db: &Database,
    paths: &DataPaths,
    name: &str,
    root: &str,
) -> Result<Project, IpcError> {
    let root = validate_root(root, paths)?;
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("Enter a project name."));
    }
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
    .map_err(IpcError::from)
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
    fn creates_projects_for_existing_folders_only() {
        let (dir, paths) = paths();
        let project_dir = dir.path().join("shop");
        std::fs::create_dir(&project_dir).unwrap();
        let db = db();

        let p = create(&db, &paths, " Shop ", &project_dir.to_string_lossy()).unwrap();
        assert_eq!(p.name, "Shop");
        assert!(!p.root_path.starts_with(r"\\?\"), "{}", p.root_path);

        let upper = project_dir.to_string_lossy().to_uppercase();
        assert!(
            create(&db, &paths, "Again", &upper)
                .unwrap_err()
                .message
                .contains("already added")
        );
        assert!(create(&db, &paths, "x", "relative\\path").is_err());
        assert!(
            create(
                &db,
                &paths,
                "x",
                &dir.path().join("missing").to_string_lossy()
            )
            .is_err()
        );
        assert!(create(&db, &paths, "x", &paths.packages.to_string_lossy()).is_err());
        assert!(create(&db, &paths, "  ", &project_dir.to_string_lossy()).is_err());
    }
}
