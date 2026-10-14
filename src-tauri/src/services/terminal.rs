//! "Open terminal" (ADR 0015): the program chosen in Settings (PowerShell, Command Prompt or
//! a Devluma-installed Cmder), in a project folder or the user's home folder, with the
//! effective runtimes first on PATH for that window only.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

use super::preferences::{self, TerminalProgram};
use super::runtime;
use crate::domain::catalog::Catalog;
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::{Database, DbResult, projects};
use crate::platform::windows::process::{self, TerminalLauncher};
use crate::state::DataPaths;

pub const CMDER_PACKAGE: &str = "cmder";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpened {
    pub program: TerminalProgram,
    pub path_prefix: Vec<String>,
}

/// `Cmder.exe` of the newest installed Cmder, if any.
pub fn cmder_exe(c: &Connection) -> DbResult<Option<PathBuf>> {
    let newest = installations::list(c)?
        .into_iter()
        .filter(|i| i.package_id == CMDER_PACKAGE && i.status == InstallStatus::Installed)
        .max_by(|a, b| version_key(&a.package_version_id).cmp(&version_key(&b.package_version_id)));
    Ok(newest
        .map(|i| Path::new(&i.install_path).join("Cmder.exe"))
        .filter(|p| p.is_file()))
}

/// Numeric parts of the version in `cmder@1.3.25@x64`.
fn version_key(version_row_id: &str) -> Vec<u64> {
    version_row_id
        .split('@')
        .nth(1)
        .unwrap_or_default()
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Where Cmder keeps its user settings (its `/C` folder), outside the package so they
/// survive upgrades and uninstalls. Passed at launch, never stored.
pub fn cmder_config_root(paths: &DataPaths) -> PathBuf {
    paths.root.join("config").join("cmder")
}

/// Opens the preferred terminal in the project's folder, or the home folder without one.
pub fn open(
    db: &Database,
    catalog: &Catalog,
    paths: &DataPaths,
    project_id: Option<&str>,
) -> Result<TerminalOpened, IpcError> {
    let (prefs, cmder, project) = db.read(|c| {
        Ok((
            preferences::load(c)?,
            cmder_exe(c)?,
            project_id.map(|id| projects::get(c, id)).transpose()?,
        ))
    })?;
    let cwd = match &project {
        Some(p) => PathBuf::from(&p.root_path),
        None => std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.root.clone()),
    };
    if !cwd.is_dir() {
        return Err(IpcError::new(
            ErrorCode::ValidationError,
            format!("The folder {} no longer exists.", cwd.display()),
            false,
        ));
    }
    let mut path_prefix = runtime::path_prefix(db, catalog, project_id)?;
    // An activated-like venv: its Scripts folder wins over the selected Python.
    if project.is_some()
        && let Some(venv) = super::projects::detect(&cwd).virtual_env_scripts
    {
        path_prefix.insert(0, venv);
    }

    let config_root = cmder_config_root(paths);
    let launcher = match prefs.terminal {
        TerminalProgram::PowerShell => TerminalLauncher::PowerShell,
        TerminalProgram::CommandPrompt => TerminalLauncher::CommandPrompt,
        TerminalProgram::Cmder => match &cmder {
            Some(exe) => TerminalLauncher::Cmder {
                exe,
                config_root: &config_root,
            },
            None => {
                return Err(IpcError::new(
                    ErrorCode::ValidationError,
                    "Cmder is chosen as the terminal but isn't installed. Install it from Runtimes & Packages, or choose another terminal in Settings.",
                    false,
                ));
            }
        },
    };
    process::open_terminal(&launcher, &cwd, &path_prefix).map_err(|_| {
        IpcError::new(
            ErrorCode::Internal,
            "Could not open a terminal window.",
            true,
        )
    })?;
    Ok(TerminalOpened {
        program: prefs.terminal,
        path_prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_versions_numerically() {
        assert!(version_key("cmder@1.3.25@x64") > version_key("cmder@1.3.9@x64"));
        assert!(version_key("cmder@1.10.0@x64") > version_key("cmder@1.9.99@x64"));
    }
}
