//! pnpm and Yarn for a Node.js installation, through the Corepack that ships with Node.js
//! (ADR 0020). `corepack enable <name> --install-directory <node folder>` writes the
//! `pnpm.cmd` / `yarn.cmd` commands into that Node.js folder only, so they are on PATH
//! exactly where that Node.js is (project terminals, scheduled jobs). Corepack downloads the
//! package manager itself on first use (from the npm registry, checked against the
//! signatures Corepack knows) into `<root>\corepack`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, IpcError};
use crate::persistence::Database;
use crate::persistence::installations::{self, InstallStatus};
use crate::platform::windows::process;
use crate::state::DataPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    Pnpm,
    Yarn,
}

impl PackageManager {
    pub const ALL: [PackageManager; 2] = [PackageManager::Pnpm, PackageManager::Yarn];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
        }
    }

    /// The command file Corepack writes for it.
    fn shim(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm.cmd",
            Self::Yarn => "yarn.cmd",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeTools {
    pub installation_id: String,
    /// This Node.js includes Corepack (Node.js 25 and later don't).
    pub corepack: bool,
    pub enabled: Vec<PackageManager>,
}

/// Corepack's cache and the no-prompt setting (the user turned the tool on themselves).
pub fn env(paths: &DataPaths) -> Vec<(String, String)> {
    vec![
        (
            "COREPACK_HOME".into(),
            paths.root.join("corepack").display().to_string(),
        ),
        ("COREPACK_ENABLE_DOWNLOAD_PROMPT".into(), "0".into()),
    ]
}

fn corepack_js(node_dir: &Path) -> PathBuf {
    node_dir
        .join("node_modules")
        .join("corepack")
        .join("dist")
        .join("corepack.js")
}

pub fn status_of(installation_id: &str, node_dir: &Path) -> NodeTools {
    NodeTools {
        installation_id: installation_id.to_owned(),
        corepack: corepack_js(node_dir).is_file(),
        enabled: PackageManager::ALL
            .into_iter()
            .filter(|pm| node_dir.join(pm.shim()).is_file())
            .collect(),
    }
}

fn node_dir(db: &Database, installation_id: &str) -> Result<PathBuf, IpcError> {
    let inst = db.read(|c| installations::get(c, installation_id))?;
    if inst.package_id != "node" || inst.status != InstallStatus::Installed {
        return Err(IpcError::new(
            ErrorCode::ValidationError,
            "That isn't an installed Node.js version.",
            false,
        ));
    }
    Ok(PathBuf::from(inst.install_path))
}

pub fn status(db: &Database, installation_id: &str) -> Result<NodeTools, IpcError> {
    Ok(status_of(installation_id, &node_dir(db, installation_id)?))
}

/// Turns pnpm / Yarn on or off for one Node.js installation.
pub fn set(
    db: &Database,
    paths: &DataPaths,
    installation_id: &str,
    enabled: &[PackageManager],
) -> Result<NodeTools, IpcError> {
    let dir = node_dir(db, installation_id)?;
    apply(paths, &dir, enabled)?;
    Ok(status_of(installation_id, &dir))
}

/// Runs `corepack enable|disable <names> --install-directory <dir>` with this Node.js.
pub fn apply(paths: &DataPaths, dir: &Path, enabled: &[PackageManager]) -> Result<(), IpcError> {
    let js = corepack_js(dir);
    if !js.is_file() {
        return Err(IpcError::new(
            ErrorCode::ValidationError,
            "This Node.js version doesn't include Corepack, so Devluma can't add pnpm or Yarn to it.",
            false,
        ));
    }
    let node = dir.join("node.exe");
    let js = js.to_string_lossy().into_owned();
    let dir_s = dir.to_string_lossy().into_owned();
    for (verb, list) in [
        (
            "enable",
            PackageManager::ALL
                .into_iter()
                .filter(|p| enabled.contains(p))
                .collect::<Vec<_>>(),
        ),
        (
            "disable",
            PackageManager::ALL
                .into_iter()
                .filter(|p| !enabled.contains(p))
                .collect(),
        ),
    ] {
        if list.is_empty() {
            continue;
        }
        let mut args = vec![js.as_str(), verb];
        args.extend(list.iter().map(|p| p.as_str()));
        args.extend(["--install-directory", &dir_s]);
        let out = process::run_captured_env(&node, &args, &env(paths), Duration::from_secs(60))
            .map_err(|e| {
                IpcError::new(
                    ErrorCode::ProcessExited,
                    format!("Could not run Corepack: {e}"),
                    true,
                )
            })?;
        if out.exit_code != Some(0) {
            return Err(IpcError::new(
                ErrorCode::ProcessExited,
                format!(
                    "Corepack {verb} failed: {}",
                    out.stderr.trim().chars().take(400).collect::<String>()
                ),
                true,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_the_commands_corepack_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let s = status_of("i1", dir.path());
        assert!(!s.corepack && s.enabled.is_empty());
        std::fs::create_dir_all(dir.path().join(r"node_modules\corepack\dist")).unwrap();
        std::fs::write(
            dir.path().join(r"node_modules\corepack\dist\corepack.js"),
            "",
        )
        .unwrap();
        std::fs::write(dir.path().join("yarn.cmd"), "").unwrap();
        let s = status_of("i1", dir.path());
        assert!(s.corepack);
        assert_eq!(s.enabled, [PackageManager::Yarn]);
    }

    #[test]
    fn missing_corepack_is_reported_not_faked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        let err = apply(&paths, dir.path(), &[PackageManager::Pnpm]).unwrap_err();
        assert!(
            err.message.contains("doesn't include Corepack"),
            "{}",
            err.message
        );
    }

    #[test]
    fn corepack_cache_stays_inside_the_root() {
        let env = env(&DataPaths::under(Path::new(r"D:\Devluma")));
        assert_eq!(env[0].1, r"D:\Devluma\corepack");
    }

    /// Real Node.js 24 from the catalog; Corepack adds and removes pnpm / Yarn, and the
    /// `pnpm` command runs (Corepack downloads pnpm from npm into `<root>\corepack`).
    /// `cargo test live_node_package_managers -- --ignored` (~34 MB plus pnpm).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network"]
    async fn live_node_package_managers() {
        use crate::services::supervisor::tests::LiveEnv;
        let env = LiveEnv::install(&[("node", "24.21.0")]).await;
        let node = env.installation("node");
        let dir = PathBuf::from(&node.install_path);
        assert!(
            status_of(&node.id, &dir).corepack,
            "Node.js 24 ships Corepack"
        );

        let s = set(
            &env.db,
            &env.paths,
            &node.id,
            &[PackageManager::Pnpm, PackageManager::Yarn],
        )
        .unwrap();
        assert_eq!(s.enabled, [PackageManager::Pnpm, PackageManager::Yarn]);
        let s = set(&env.db, &env.paths, &node.id, &[PackageManager::Pnpm]).unwrap();
        assert_eq!(s.enabled, [PackageManager::Pnpm], "yarn.cmd removed");

        // The command works the way a Devluma terminal runs it: Node.js first on PATH.
        let system = std::env::var("SystemRoot").unwrap();
        let mut cmd = std::process::Command::new(format!(r"{system}\System32\cmd.exe"));
        cmd.args(["/d", "/c", "pnpm", "--version"])
            .current_dir(&dir)
            .env(
                "PATH",
                format!(r"{};{system}\System32;{system}", dir.display()),
            );
        for (k, v) in super::env(&env.paths) {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            version.split('.').count() == 3 && version.chars().next().unwrap().is_ascii_digit(),
            "pnpm --version printed {version:?}"
        );
        assert!(
            env.paths.root.join("corepack").is_dir(),
            "cache inside the root"
        );
        eprintln!("pnpm {version} via Corepack");
    }
}
