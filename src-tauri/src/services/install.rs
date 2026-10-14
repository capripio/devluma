//! Package installation pipeline (trd.md §5, app_flow.md §4).
//!
//! `InstallManager::start` runs fast preflight checks and returns an operation id; the
//! work then continues in the background, journaled phase by phase:
//!
//! 1. check disk space
//! 2. record intent (`installing`)
//! 3. download to staging, hashing while streaming
//! 4. verify SHA-256
//! 5. extract safely
//! 6. run each listed executable via the adapter's version probe
//! 7. move into `packages/` and commit `installed`
//!
//! Any failure or cancellation removes staging files and undoes the intent row, so an
//! unverified package is never marked installed and a retry never duplicates records.
//! Existing versions and global defaults are never touched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::download::{self, DownloadError, DownloadOptions};
use super::journal::{Journal, ProgressSink};
use crate::domain::adapter::AdapterId;
use crate::domain::catalog::{
    Architecture, Catalog, CatalogPackage, IntegrityPolicy, VersionManifest, version_row_id,
};
use crate::error::{ErrorCode, IpcError};
use crate::persistence::installations::{self, InstallStatus};
use crate::persistence::operations::{EventLevel, NewOperation};
use crate::persistence::packages::{self, Package, PackageVersion};
use crate::persistence::{Database, DbError};
use crate::security::archive::{self, ExtractError, ExtractLimits};
use crate::security::paths;
use crate::state::DataPaths;

pub const OPERATION_TYPE: &str = "install_package";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// Download + extracted copy + headroom.
const DISK_FACTOR: u64 = 4;

/// Runs an executable with fixed arguments and returns its combined output.
pub trait ExecutableProbe: Send + Sync {
    fn run(&self, exe: &Path, args: &[&str]) -> Result<String, String>;

    /// The executable's fixed file version (`a.b.c.d`), read without running it.
    fn file_version(&self, _exe: &Path) -> Result<String, String> {
        Err("reading version resources isn't supported here".into())
    }
}

pub struct ProcessProbe;

impl ExecutableProbe for ProcessProbe {
    fn run(&self, exe: &Path, args: &[&str]) -> Result<String, String> {
        let out = crate::platform::windows::process::run_captured(exe, args, PROBE_TIMEOUT)
            .map_err(|e| e.to_string())?;
        Ok(format!("{}\n{}", out.stdout, out.stderr))
    }

    fn file_version(&self, exe: &Path) -> Result<String, String> {
        crate::platform::windows::version_info::file_version(exe).map_err(|e| e.to_string())
    }
}

/// What `adapter.parse_version` reads for `exe`: its probe output, or its version resource.
pub fn probe_output(
    probe: &dyn ExecutableProbe,
    adapter: AdapterId,
    exe: &Path,
) -> Result<String, String> {
    if adapter.reads_version_resource() {
        probe.file_version(exe).map(|v| format!("FileVersion {v}"))
    } else {
        probe.run(exe, adapter.version_probe().args)
    }
}

#[derive(Clone)]
pub struct InstallContext {
    pub db: Arc<Database>,
    pub catalog: Arc<Catalog>,
    pub paths: DataPaths,
    pub probe: Arc<dyn ExecutableProbe>,
    pub sink: Arc<dyn ProgressSink>,
    pub download: DownloadOptions,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequest {
    pub package_id: String,
    pub version: String,
    pub architecture: Architecture,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallStarted {
    pub operation_id: String,
}

struct Running {
    version_key: String,
    token: CancellationToken,
    flag: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct InstallManager {
    running: Mutex<HashMap<String, Running>>,
}

impl InstallManager {
    /// Validates the request, journals intent and starts the background task.
    pub fn start(
        self: &Arc<Self>,
        ctx: InstallContext,
        req: InstallRequest,
    ) -> Result<(InstallStarted, tauri::async_runtime::JoinHandle<()>), IpcError> {
        let (pkg, version) = preflight(&ctx, &req)?;
        let key = version_row_id(&pkg.manifest.id, &version.version, version.architecture);

        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if running.values().any(|r| r.version_key == key) {
            return Err(IpcError::new(
                ErrorCode::ValidationError,
                "This version is already being installed.",
                false,
            ));
        }
        let journal = Journal::new(&ctx.db, ctx.sink.as_ref());
        let op = journal
            .start(&NewOperation {
                operation_type: OPERATION_TYPE,
                target_type: Some("package_version"),
                target_id: Some(&key),
            })
            .map_err(IpcError::from)?;
        let token = CancellationToken::new();
        let flag = Arc::new(AtomicBool::new(false));
        running.insert(
            op.id.clone(),
            Running {
                version_key: key,
                token: token.clone(),
                flag: flag.clone(),
            },
        );
        drop(running);

        let manager = Arc::clone(self);
        let op_id = op.id.clone();
        let (pkg, version) = (pkg.clone(), version.clone());
        let handle = tauri::async_runtime::spawn(async move {
            run(&ctx, &op_id, &pkg, &version, &token, &flag).await;
            manager
                .running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&op_id);
        });
        Ok((
            InstallStarted {
                operation_id: op.id,
            },
            handle,
        ))
    }

    /// Requests cancellation. Returns false if the operation isn't a running install.
    pub fn cancel(&self, operation_id: &str) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        match running.get(operation_id) {
            Some(r) => {
                r.flag.store(true, Ordering::Relaxed);
                r.token.cancel();
                true
            }
            None => false,
        }
    }
}

fn preflight<'c>(
    ctx: &'c InstallContext,
    req: &InstallRequest,
) -> Result<(&'c CatalogPackage, &'c VersionManifest), IpcError> {
    let invalid = |msg: String| IpcError::new(ErrorCode::ValidationError, msg, false);
    let (pkg, version) = ctx
        .catalog
        .find_version(&req.package_id, &req.version, req.architecture)
        .ok_or_else(|| invalid("That package version is not in the catalog.".into()))?;
    if !version.architecture.runs_on_host() {
        return Err(invalid(format!(
            "{} {} is built for {}, which this computer can't run.",
            pkg.manifest.display_name,
            version.version,
            version.architecture.as_str()
        )));
    }
    if let Some(product) = missing_prerequisite(pkg.adapter) {
        return Err(invalid(format!(
            "{} needs the {product}, which isn't installed on this computer. Install it from Microsoft, then try again.",
            pkg.manifest.display_name
        )));
    }
    if version.integrity.policy == IntegrityPolicy::Sha256AndSignature {
        return Err(IpcError::new(
            ErrorCode::IntegrityError,
            "This package requires publisher-signature verification, which isn't available in this build of Devluma. It was not installed.",
            false,
        ));
    }

    let installed = ctx.db.read(installations::list).map_err(IpcError::from)?;
    let installed_pkg = |id: &str| {
        installed
            .iter()
            .any(|i| i.package_id == id && i.status == InstallStatus::Installed)
    };
    let key = version_row_id(&pkg.manifest.id, &version.version, version.architecture);
    if installed
        .iter()
        .any(|i| i.package_version_id == key && i.status == InstallStatus::Installed)
    {
        return Err(invalid(format!(
            "{} {} is already installed.",
            pkg.manifest.display_name, version.version
        )));
    }
    if let Some(dep) = version.dependencies.iter().find(|d| !installed_pkg(d)) {
        let name = ctx
            .catalog
            .package(dep)
            .map_or(dep.as_str(), |p| p.manifest.display_name.as_str());
        return Err(invalid(format!(
            "Install {name} first; this package depends on it."
        )));
    }
    if let Some(conflict) = version.conflicts.iter().find(|c| installed_pkg(c)) {
        let name = ctx
            .catalog
            .package(conflict)
            .map_or(conflict.as_str(), |p| p.manifest.display_name.as_str());
        return Err(invalid(format!(
            "This package conflicts with the installed {name}."
        )));
    }
    Ok((pkg, version))
}

/// The first required system component that's missing, by product name.
pub fn missing_prerequisite(adapter: AdapterId) -> Option<&'static str> {
    let system32 = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32"))?;
    adapter
        .required_system_dlls()
        .iter()
        .find(|(dll, _)| !system32.join(dll).is_file())
        .map(|(_, product)| *product)
}

/// A failure with a user-safe message.
struct Failure {
    code: ErrorCode,
    message: String,
}

enum Outcome {
    Failed(Failure),
    Cancelled,
}

impl From<DbError> for Outcome {
    fn from(e: DbError) -> Self {
        let ipc = IpcError::from(e);
        Outcome::Failed(Failure {
            code: ipc.code,
            message: ipc.message,
        })
    }
}

fn fail(code: ErrorCode, message: impl Into<String>) -> Outcome {
    Outcome::Failed(Failure {
        code,
        message: message.into(),
    })
}

/// Free space required to install an archive of `size` bytes (download + extraction + slack).
fn disk_needed(size: u64) -> u64 {
    size.saturating_mul(DISK_FACTOR)
}

fn io_failure(context: &str, e: std::io::Error) -> Outcome {
    if matches!(e.raw_os_error(), Some(112) | Some(39)) {
        fail(
            ErrorCode::DiskSpaceLow,
            format!("Ran out of disk space while {context}."),
        )
    } else {
        fail(
            ErrorCode::Internal,
            format!("A file operation failed while {context}."),
        )
    }
}

/// Where a version lives once installed: `packages/<id>/<version>-<arch>`.
pub fn install_dir(
    paths: &DataPaths,
    package_id: &str,
    version: &str,
    arch: Architecture,
) -> PathBuf {
    paths
        .packages
        .join(package_id)
        .join(format!("{version}-{}", arch.as_str()))
}

async fn run(
    ctx: &InstallContext,
    op_id: &str,
    pkg: &CatalogPackage,
    version: &VersionManifest,
    token: &CancellationToken,
    flag: &Arc<AtomicBool>,
) {
    let journal = Journal::new(&ctx.db, ctx.sink.as_ref());
    let staging = ctx.paths.staging.join(op_id);
    let mut intent: Option<(String, Option<InstallStatus>)> = None;

    let result = pipeline(
        ctx,
        &journal,
        op_id,
        pkg,
        version,
        &staging,
        token,
        flag,
        &mut intent,
    )
    .await;

    // Staging is app-managed scratch space: always removed (best effort).
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    // If the journal can't record the end state, the operation stays unfinished and startup
    // reconciliation flags it as needing recovery — so these results can be ignored.
    let _ = match result {
        Ok(()) => journal.succeed(op_id).map(|_| ()),
        Err(outcome) => {
            if let Some((id, previous)) = &intent {
                let _ = ctx.db.write(|c| installations::abandon(c, id, *previous));
            }
            match outcome {
                Outcome::Cancelled => journal.cancel(op_id).map(|_| ()),
                Outcome::Failed(f) => journal.fail(op_id, f.code, &f.message).map(|_| ()),
            }
        }
    };
}

#[allow(clippy::too_many_arguments)]
async fn pipeline(
    ctx: &InstallContext,
    journal: &Journal<'_>,
    op_id: &str,
    pkg: &CatalogPackage,
    version: &VersionManifest,
    staging: &Path,
    token: &CancellationToken,
    flag: &Arc<AtomicBool>,
    intent: &mut Option<(String, Option<InstallStatus>)>,
) -> Result<(), Outcome> {
    let check_cancel = || {
        if token.is_cancelled() {
            Err(Outcome::Cancelled)
        } else {
            Ok(())
        }
    };
    let size = version.source.size_bytes;
    let key = version_row_id(&pkg.manifest.id, &version.version, version.architecture);
    let final_dir = install_dir(
        &ctx.paths,
        &pkg.manifest.id,
        &version.version,
        version.architecture,
    );

    journal.running(op_id)?;

    // 1. Disk space
    journal.progress(op_id, "Checking disk space", None, None)?;
    std::fs::create_dir_all(staging).map_err(|e| io_failure("preparing the staging folder", e))?;
    let needed = disk_needed(size);
    match crate::platform::windows::disk::available_bytes(staging) {
        Ok(available) if available < needed => {
            return Err(fail(
                ErrorCode::DiskSpaceLow,
                format!(
                    "Needs about {} MB free in {}; {} MB available.",
                    needed / 1_048_576,
                    ctx.paths.root.display(),
                    available / 1_048_576
                ),
            ));
        }
        _ => {}
    }
    check_cancel()?;

    // 2. Record intent before any side effect outside staging.
    let install_path = final_dir.to_string_lossy().into_owned();
    let (installation, previous) = ctx.db.write(|c| {
        persist_catalog_rows(c, &ctx.catalog.catalog_version, pkg, version)?;
        installations::begin_or_resume(c, &key, &install_path)
    })?;
    *intent = Some((installation.id.clone(), previous));
    ctx.db.write(|c| {
        crate::persistence::operations::set_recovery_data(
            c,
            op_id,
            &serde_json::json!({
                "stagingDir": staging,
                "installPath": install_path,
                "installationId": installation.id,
            }),
        )
    })?;

    // 3. Download
    let archive_path = staging.join("download.zip");
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut last_pct = u64::MAX;
    let downloaded = download::download(
        &version.source.url,
        &pkg.manifest.allowed_hosts,
        size,
        &archive_path,
        &ctx.download,
        token,
        |received, total| {
            let pct = received * 100 / total.max(1);
            if pct != last_pct
                && (last_emit.elapsed() >= Duration::from_millis(250) || received == total)
            {
                last_pct = pct;
                last_emit = Instant::now();
                let _ = journal.progress(
                    op_id,
                    "Downloading",
                    Some(received as i64),
                    Some(total as i64),
                );
            }
        },
    )
    .await
    .map_err(download_failure)?;
    check_cancel()?;

    // 4. Integrity
    journal.progress(op_id, "Verifying checksum", None, None)?;
    if downloaded.sha256 != version.integrity.sha256 {
        return Err(fail(
            ErrorCode::IntegrityError,
            format!(
                "Checksum mismatch: expected SHA-256 {}, got {}. The download was discarded.",
                version.integrity.sha256, downloaded.sha256
            ),
        ));
    }
    journal.event(
        op_id,
        EventLevel::Info,
        "SHA-256 verified",
        &serde_json::json!({ "sha256": downloaded.sha256, "bytes": downloaded.bytes }),
    )?;

    // 5. Extract (blocking work off the async runtime)
    journal.progress(op_id, "Extracting", None, None)?;
    let extract_dir = staging.join("extract");
    let limits = ExtractLimits::for_archive(size, version.source.strip_components);
    let (archive_clone, extract_clone, flag_clone) =
        (archive_path.clone(), extract_dir.clone(), flag.clone());
    tauri::async_runtime::spawn_blocking(move || {
        archive::extract_zip(&archive_clone, &extract_clone, limits, &flag_clone)
    })
    .await
    .map_err(|_| fail(ErrorCode::Internal, "Extraction stopped unexpectedly."))?
    .map_err(extract_failure)?;
    check_cancel()?;

    // 6. Verify executables using the adapter's fixed probe.
    journal.progress(op_id, "Verifying executables", None, None)?;
    verify_executables(ctx, pkg.adapter, version, &extract_dir).await?;
    check_cancel()?;

    // 7. Move into place and commit.
    journal.progress(op_id, "Installing", None, None)?;
    if final_dir.exists() {
        // Leftover from an interrupted attempt; this version isn't installed (checked above),
        // and the path is ours — but confirm it really is under packages/ before deleting.
        paths::ensure_within(&ctx.paths.packages, &final_dir).map_err(|_| {
            fail(
                ErrorCode::Internal,
                "Refusing to remove a folder outside the packages directory.",
            )
        })?;
        std::fs::remove_dir_all(&final_dir)
            .map_err(|e| io_failure("removing a previous incomplete install", e))?;
    }
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| io_failure("creating the package folder", e))?;
    }
    std::fs::rename(&extract_dir, &final_dir)
        .map_err(|e| io_failure("moving files into place", e))?;
    let committed = ctx.db.write(|c| {
        installations::mark_installed(c, &installation.id, "sha256 verified; executables verified")
    });
    if let Err(e) = committed {
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(e.into());
    }
    Ok(())
}

pub(crate) fn persist_catalog_rows(
    c: &rusqlite::Connection,
    catalog_version: &str,
    pkg: &CatalogPackage,
    v: &VersionManifest,
) -> crate::persistence::DbResult<()> {
    let m = &pkg.manifest;
    packages::upsert(
        c,
        &Package {
            id: m.id.clone(),
            display_name: m.display_name.clone(),
            category: serde_json::to_value(m.category)?
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            description: m.description.clone(),
            publisher: Some(m.publisher.clone()),
            homepage_url: m.homepage_url.clone(),
            license_id: Some(m.license.id.clone()),
            license_url: m.license.url.clone(),
            catalog_source: "bundled".into(),
            catalog_version: catalog_version.into(),
            support_policy_url: m.support_policy_url.clone(),
        },
    )?;
    packages::upsert_version(
        c,
        &PackageVersion {
            id: version_row_id(&m.id, &v.version, v.architecture),
            package_id: m.id.clone(),
            version: v.version.clone(),
            architecture: v.architecture.as_str().into(),
            os_constraints: serde_json::json!({ "minWindowsBuild": v.min_windows_build }),
            source_url: v.source.url.clone(),
            publisher_signature_url: None,
            sha256: Some(v.integrity.sha256.clone()),
            integrity_policy: v.integrity.policy.as_str().into(),
            archive_type: "zip".into(),
            expected_size_bytes: Some(v.source.size_bytes as i64),
            release_date: v.release_date.clone(),
            end_of_life_date: v.end_of_life_date.clone(),
            manifest: serde_json::json!({
                "executables": v.executables,
                "dependencies": v.dependencies,
                "conflicts": v.conflicts,
                "stripComponents": v.source.strip_components,
            }),
        },
    )
}

async fn verify_executables(
    ctx: &InstallContext,
    adapter: AdapterId,
    version: &VersionManifest,
    root: &Path,
) -> Result<(), Outcome> {
    for exe in &version.executables {
        let path = paths::join_relative(root, &exe.path).map_err(|_| {
            fail(
                ErrorCode::IntegrityError,
                format!("Invalid executable path {}.", exe.path),
            )
        })?;
        if !path.is_file() {
            return Err(fail(
                ErrorCode::IntegrityError,
                format!(
                    "Expected executable {} is missing from the package.",
                    exe.path
                ),
            ));
        }
        let probe_impl = Arc::clone(&ctx.probe);
        let output = tauri::async_runtime::spawn_blocking(move || {
            probe_output(&*probe_impl, adapter, &path)
        })
        .await
        .map_err(|_| {
            fail(
                ErrorCode::Internal,
                "Executable verification stopped unexpectedly.",
            )
        })?
        .map_err(|e| {
            fail(
                ErrorCode::IntegrityError,
                format!("Could not run {}: {e}", exe.path),
            )
        })?;
        match adapter.parse_version(&output) {
            Some(found) if found == exe.expected_version => {}
            Some(found) => {
                return Err(fail(
                    ErrorCode::IntegrityError,
                    format!(
                        "{} reported version {found}, expected {}.",
                        exe.path, exe.expected_version
                    ),
                ));
            }
            None => {
                return Err(fail(
                    ErrorCode::IntegrityError,
                    format!("{} did not report a recognizable version.", exe.path),
                ));
            }
        }
    }
    Ok(())
}

fn download_failure(e: DownloadError) -> Outcome {
    use DownloadError::*;
    match e {
        Cancelled => Outcome::Cancelled,
        HostNotAllowed(_) => fail(
            ErrorCode::IntegrityError,
            format!("{e}. The source policy blocked it."),
        ),
        Network(_) | HttpStatus(_) | Stalled(_) => {
            fail(ErrorCode::NetworkError, format!("Download failed: {e}."))
        }
        TooLarge { .. } | SizeMismatch { .. } => fail(
            ErrorCode::IntegrityError,
            format!("{e}. The download was discarded."),
        ),
        DiskFull => fail(ErrorCode::DiskSpaceLow, e.to_string()),
        Io(io) => io_failure("saving the download", io),
    }
}

fn extract_failure(e: ExtractError) -> Outcome {
    match e {
        ExtractError::Cancelled => Outcome::Cancelled,
        ExtractError::Io(io) => io_failure("extracting the archive", io),
        other => fail(
            ErrorCode::IntegrityError,
            format!("The archive was rejected: {other}."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::domain::catalog::{ValidationOptions, fixtures, parse_and_validate};
    use crate::domain::operation::OperationStatus;
    use crate::persistence::operations as ops;
    use crate::security::archive::test_zip;
    use crate::services::download::test_server::{Route, TestServer, serve};
    use crate::services::journal::test_support::RecordingSink;

    /// Reports whatever version each executable path was registered with.
    #[derive(Default)]
    struct FakeProbe(StdMutex<Vec<(String, String)>>);

    #[test]
    fn version_resources_are_read_instead_of_running_the_program() {
        struct ResourceOnly;
        impl ExecutableProbe for ResourceOnly {
            fn run(&self, _: &Path, _: &[&str]) -> Result<String, String> {
                panic!("Cmder.exe must not be run");
            }
            fn file_version(&self, _: &Path) -> Result<String, String> {
                Ok("1.3.25.328".into())
            }
        }
        let out = probe_output(&ResourceOnly, AdapterId::Cmder, Path::new("Cmder.exe")).unwrap();
        assert_eq!(
            AdapterId::Cmder.parse_version(&out).as_deref(),
            Some("1.3.25")
        );
        // Probes without version-resource support refuse rather than guess.
        assert!(
            probe_output(
                &FakeProbe(StdMutex::new(vec![])),
                AdapterId::Cmder,
                Path::new("Cmder.exe")
            )
            .is_err()
        );
    }

    impl ExecutableProbe for FakeProbe {
        fn run(&self, exe: &Path, _args: &[&str]) -> Result<String, String> {
            let name = exe.file_name().unwrap().to_string_lossy().into_owned();
            self.0
                .lock()
                .unwrap()
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, out)| out.clone())
                .ok_or_else(|| "not executable".into())
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        _server: TestServer,
        ctx: InstallContext,
        manager: Arc<InstallManager>,
        sink: Arc<RecordingSink>,
    }

    fn tool_zip() -> Vec<u8> {
        test_zip::build(&[
            ("tool-1.2.3/", b""),
            ("tool-1.2.3/bin/tool.exe", b"MZ fake"),
            ("tool-1.2.3/LICENSE", b"MIT"),
        ])
    }

    /// Serves `zip` and builds a catalog whose single version points at it.
    fn harness(
        zip: Vec<u8>,
        tweak: impl FnOnce(&mut serde_json::Value),
        probe_output: &str,
    ) -> Harness {
        let server = serve(vec![
            ("/tool.zip", Route::Bytes(zip.clone())),
            (
                "/slow.zip",
                Route::Stall {
                    head: zip[..10].to_vec(),
                    total_len: zip.len(),
                    pause: Duration::from_secs(10),
                },
            ),
        ]);
        let mut v = fixtures::version("1.2.3");
        v["source"] = json!({
            "url": format!("{}/tool.zip", server.base),
            "archive": "zip",
            "sizeBytes": zip.len(),
            "stripComponents": 1
        });
        v["integrity"]["sha256"] = json!(hex::encode(Sha256::digest(&zip)));
        tweak(&mut v);
        let mut p = fixtures::package("tool", vec![v]);
        p["allowedHosts"] = json!(["127.0.0.1"]);
        let catalog = parse_and_validate(
            &fixtures::document(vec![p]),
            ValidationOptions {
                allow_loopback_http: true,
            },
        )
        .unwrap();
        assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);

        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let probe = FakeProbe::default();
        probe
            .0
            .lock()
            .unwrap()
            .push(("tool.exe".into(), probe_output.into()));
        let sink = Arc::new(RecordingSink::default());
        Harness {
            ctx: InstallContext {
                db: Arc::new(crate::persistence::test_support::db()),
                catalog: Arc::new(catalog),
                paths,
                probe: Arc::new(probe),
                sink: sink.clone(),
                download: DownloadOptions {
                    allow_loopback_http: true,
                    idle_timeout: Duration::from_secs(5),
                    ..Default::default()
                },
            },
            _dir: dir,
            _server: server,
            manager: Arc::new(InstallManager::default()),
            sink,
        }
    }

    fn req() -> InstallRequest {
        InstallRequest {
            package_id: "tool".into(),
            version: "1.2.3".into(),
            architecture: Architecture::X64,
        }
    }

    async fn install(h: &Harness) -> ops::Operation {
        let (started, handle) = h.manager.start(h.ctx.clone(), req()).unwrap();
        handle.await.unwrap();
        h.ctx
            .db
            .read(|c| ops::get(c, &started.operation_id))
            .unwrap()
    }

    fn installations(h: &Harness) -> Vec<installations::Installation> {
        h.ctx.db.read(installations::list).unwrap()
    }

    fn staging_is_empty(h: &Harness) -> bool {
        std::fs::read_dir(&h.ctx.paths.staging)
            .unwrap()
            .next()
            .is_none()
    }

    fn final_dir(h: &Harness) -> PathBuf {
        install_dir(&h.ctx.paths, "tool", "1.2.3", Architecture::X64)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn installs_end_to_end() {
        let h = harness(tool_zip(), |_| {}, "fixture-tool 1.2.3");
        let op = install(&h).await;
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );

        let inst = installations(&h);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].status, InstallStatus::Installed);
        assert!(inst[0].verified_at.is_some());
        assert!(
            !inst[0].is_global_default,
            "installing never changes the default"
        );
        assert_eq!(
            std::fs::read(final_dir(&h).join("bin").join("tool.exe")).unwrap(),
            b"MZ fake"
        );
        assert!(staging_is_empty(&h));

        let phases: Vec<_> = h
            .sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.phase.clone())
            .collect();
        for expected in [
            "Checking disk space",
            "Downloading",
            "Verifying checksum",
            "Extracting",
            "Verifying executables",
            "Installing",
        ] {
            assert!(
                phases.iter().any(|p| p == expected),
                "missing phase {expected}: {phases:?}"
            );
        }
    }

    async fn assert_rejected(h: &Harness, code: &str, message_contains: &str) {
        let op = install(h).await;
        assert_eq!(op.status, OperationStatus::Failed);
        assert_eq!(
            op.error_code.as_deref(),
            Some(code),
            "{:?}",
            op.safe_error_message
        );
        let msg = op.safe_error_message.unwrap();
        assert!(msg.contains(message_contains), "{msg}");
        assert!(
            installations(h).is_empty(),
            "nothing may be recorded as installed"
        );
        assert!(!final_dir(h).exists(), "nothing may be placed in packages/");
        assert!(staging_is_empty(h), "staging must be cleaned up");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn checksum_mismatch_is_never_installed() {
        let h = harness(
            tool_zip(),
            |v| v["integrity"]["sha256"] = json!("b".repeat(64)),
            "fixture-tool 1.2.3",
        );
        assert_rejected(&h, "INTEGRITY_ERROR", "Checksum mismatch").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_executable_version_is_never_installed() {
        let h = harness(tool_zip(), |_| {}, "fixture-tool 9.9.9");
        assert_rejected(&h, "INTEGRITY_ERROR", "reported version 9.9.9").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_executable_is_never_installed() {
        let zip = test_zip::build(&[("tool-1.2.3/README", b"no exe here")]);
        let h = harness(zip, |_| {}, "fixture-tool 1.2.3");
        assert_rejected(&h, "INTEGRITY_ERROR", "missing").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn malicious_archive_is_never_installed() {
        let zip = test_zip::build(&[
            ("tool-1.2.3/bin/tool.exe", b"MZ"),
            ("tool-1.2.3/../../escape.txt", b"x"),
        ]);
        let h = harness(zip, |_| {}, "fixture-tool 1.2.3");
        assert_rejected(&h, "INTEGRITY_ERROR", "archive was rejected").await;
        assert!(!h.ctx.paths.root.join("escape.txt").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn network_failure_is_reported_and_retry_succeeds_without_duplicates() {
        let mut h = harness(tool_zip(), |_| {}, "fixture-tool 1.2.3");
        // Point at a missing path first.
        let good = h.ctx.catalog.as_ref().clone();
        let mut broken = good.clone();
        broken.packages[0].versions[0].source.url = broken.packages[0].versions[0]
            .source
            .url
            .replace("tool.zip", "missing.zip");
        h.ctx.catalog = Arc::new(broken);
        assert_rejected(&h, "NETWORK_ERROR", "HTTP 404").await;

        h.ctx.catalog = Arc::new(good);
        assert_eq!(install(&h).await.status, OperationStatus::Succeeded);
        assert_eq!(installations(&h).len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn preflight_rejects_duplicates_and_unsupported_policies() {
        let h = harness(tool_zip(), |_| {}, "fixture-tool 1.2.3");
        assert_eq!(install(&h).await.status, OperationStatus::Succeeded);
        let err = h.manager.start(h.ctx.clone(), req()).err().unwrap();
        assert!(err.message.contains("already installed"), "{}", err.message);

        let h = harness(
            tool_zip(),
            |v| {
                v["integrity"]["policy"] = json!("sha256+signature");
                v["integrity"]["signature"] =
                    json!({ "kind": "authenticode", "signer": "Fixture Inc." });
            },
            "fixture-tool 1.2.3",
        );
        let err = h.manager.start(h.ctx.clone(), req()).err().unwrap();
        assert_eq!(err.code, ErrorCode::IntegrityError);
        assert!(installations(&h).is_empty());

        let err = h
            .manager
            .start(
                h.ctx.clone(),
                InstallRequest {
                    version: "0.0.1".into(),
                    ..req()
                },
            )
            .err()
            .unwrap();
        assert_eq!(err.code, ErrorCode::ValidationError);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_cleans_up_and_concurrent_duplicates_are_refused() {
        let mut h = harness(tool_zip(), |_| {}, "fixture-tool 1.2.3");
        let mut slow = h.ctx.catalog.as_ref().clone();
        slow.packages[0].versions[0].source.url = slow.packages[0].versions[0]
            .source
            .url
            .replace("tool.zip", "slow.zip");
        h.ctx.catalog = Arc::new(slow);

        let (started, handle) = h.manager.start(h.ctx.clone(), req()).unwrap();
        let dup = h.manager.start(h.ctx.clone(), req()).err().unwrap();
        assert!(dup.message.contains("already being installed"));

        // Wait until the download has begun, then cancel.
        for _ in 0..100 {
            if h.sink
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.phase.as_deref() == Some("Downloading"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(h.manager.cancel(&started.operation_id));
        handle.await.unwrap();

        let op = h
            .ctx
            .db
            .read(|c| ops::get(c, &started.operation_id))
            .unwrap();
        assert_eq!(op.status, OperationStatus::Cancelled);
        assert!(installations(&h).is_empty());
        assert!(staging_is_empty(&h));
        assert!(
            !h.manager.cancel(&started.operation_id),
            "finished operations can't be cancelled"
        );
    }

    /// Real end-to-end install of PHP from the bundled catalog: official download, SHA-256,
    /// extraction and running the real `php.exe`/`php-cgi.exe` probes.
    /// `cargo test live_php_install -- --ignored` (downloads ~35 MB).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network"]
    async fn live_php_install_from_bundled_catalog() {
        let catalog = crate::services::catalog::load();
        assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let ctx = InstallContext {
            db: Arc::new(crate::persistence::test_support::db()),
            catalog: Arc::new(catalog),
            paths,
            probe: Arc::new(ProcessProbe),
            sink: Arc::new(RecordingSink::default()),
            download: DownloadOptions::default(),
        };
        let manager = Arc::new(InstallManager::default());
        let (started, handle) = manager
            .start(
                ctx.clone(),
                InstallRequest {
                    package_id: "php".into(),
                    version: "8.4.26".into(),
                    architecture: Architecture::X64,
                },
            )
            .unwrap();
        handle.await.unwrap();
        let op = ctx.db.read(|c| ops::get(c, &started.operation_id)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
        let dir = install_dir(&ctx.paths, "php", "8.4.26", Architecture::X64);
        assert!(dir.join("php.exe").is_file() && dir.join("php-cgi.exe").is_file());

        // php.ini from the official template validates with the real php.exe, and enabling an
        // extension works because extension_dir points at this install's own ext folder.
        use crate::services::php;
        php::create_ini(&ProcessProbe, &dir, php::IniTemplate::Development).unwrap();
        let ini = dir.join(php::INI_FILE);
        let text = std::fs::read_to_string(&ini)
            .unwrap()
            .replace(";extension=curl", "extension=curl");
        std::fs::write(&ini, text).unwrap();
        let details = php::inspect(&ProcessProbe, &dir).unwrap();
        assert!(details.ini_exists);
        assert!(
            details.startup_warnings.is_empty(),
            "{:?}",
            details.startup_warnings
        );
        assert!(
            details.loaded_modules.iter().any(|m| m == "curl"),
            "{:?}",
            details.loaded_modules
        );
        assert!(
            details
                .available_extensions
                .contains(&"mbstring".to_owned())
        );
    }

    /// Cmder from the bundled catalog: verified by its version resource (Cmder.exe is never
    /// run). `cargo test live_cmder_install -- --ignored` (downloads ~12 MB).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network"]
    async fn live_cmder_install_from_bundled_catalog() {
        let catalog = crate::services::catalog::load();
        assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let ctx = InstallContext {
            db: Arc::new(crate::persistence::test_support::db()),
            catalog: Arc::new(catalog),
            paths,
            probe: Arc::new(ProcessProbe),
            sink: Arc::new(RecordingSink::default()),
            download: DownloadOptions::default(),
        };
        let manager = Arc::new(InstallManager::default());
        let (started, handle) = manager
            .start(
                ctx.clone(),
                InstallRequest {
                    package_id: "cmder".into(),
                    version: "1.3.25".into(),
                    architecture: Architecture::X64,
                },
            )
            .unwrap();
        handle.await.unwrap();
        let op = ctx.db.read(|c| ops::get(c, &started.operation_id)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
        let exe = ctx
            .db
            .read(crate::services::terminal::cmder_exe)
            .unwrap()
            .expect("installed Cmder is found");
        assert_eq!(
            exe,
            install_dir(&ctx.paths, "cmder", "1.3.25", Architecture::X64).join("Cmder.exe")
        );
        assert!(
            exe.with_file_name("vendor")
                .join("conemu-maximus5")
                .join("ConEmu64.exe")
                .is_file()
        );
    }

    /// Real installs of Node.js and Python from the bundled catalog, selected as defaults:
    /// project terminals get the right folders on PATH, npm runs, and a venv created by the
    /// installed Python is detected. `cargo test live_node_and_python -- --ignored` (~55 MB).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "uses the network"]
    async fn live_node_and_python_runtimes() {
        use crate::services::runtime::{self, Scope, SwitchRequest};
        let catalog = crate::services::catalog::load();
        assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(&dir.path().join("Devluma"));
        paths.create_all().unwrap();
        let ctx = InstallContext {
            db: Arc::new(crate::persistence::test_support::db()),
            catalog: Arc::new(catalog),
            paths: paths.clone(),
            probe: Arc::new(ProcessProbe),
            sink: Arc::new(RecordingSink::default()),
            download: DownloadOptions::default(),
        };
        let manager = Arc::new(InstallManager::default());
        for (package, version) in [("node", "24.21.0"), ("python", "3.14.8")] {
            let (started, handle) = manager
                .start(
                    ctx.clone(),
                    InstallRequest {
                        package_id: package.into(),
                        version: version.into(),
                        architecture: Architecture::X64,
                    },
                )
                .unwrap();
            handle.await.unwrap();
            let op = ctx.db.read(|c| ops::get(c, &started.operation_id)).unwrap();
            assert_eq!(
                op.status,
                OperationStatus::Succeeded,
                "{package}: {:?}",
                op.safe_error_message
            );
        }
        let node_dir = install_dir(&paths, "node", "24.21.0", Architecture::X64);
        let py_dir = install_dir(&paths, "python", "3.14.8", Architecture::X64);
        assert!(
            node_dir.join("npm.cmd").is_file() && py_dir.join("tools").join("python.exe").is_file()
        );

        let probe: Arc<dyn ExecutableProbe> = Arc::new(ProcessProbe);
        for package in ["node", "python"] {
            let inst = ctx
                .db
                .read(installations::list)
                .unwrap()
                .into_iter()
                .find(|i| i.package_id == package)
                .unwrap();
            runtime::apply(
                &ctx.db,
                &ctx.catalog,
                &probe,
                ctx.sink.as_ref(),
                &SwitchRequest {
                    package_id: package.into(),
                    scope: Scope::Global,
                    project_id: None,
                    installation_id: Some(inst.id),
                },
            )
            .unwrap();
        }

        let project_dir = dir.path().join("api");
        std::fs::create_dir(&project_dir).unwrap();
        std::fs::write(project_dir.join("pyproject.toml"), "").unwrap();
        let project = crate::services::projects::create(
            &ctx.db,
            &paths,
            "API",
            &project_dir.to_string_lossy(),
        )
        .unwrap();
        let prefix = runtime::project_path_prefix(&ctx.db, &ctx.catalog, &project.id).unwrap();
        let tools = py_dir.join("tools");
        let expected = [
            node_dir.to_string_lossy().into_owned(),
            tools.to_string_lossy().into_owned(),
            tools.join("Scripts").to_string_lossy().into_owned(),
        ];
        for e in &expected {
            assert!(prefix.contains(e), "{e} missing from {prefix:?}");
        }

        // npm (a .cmd shim) runs with the selected Node.js first on PATH.
        let system = std::env::var("SystemRoot").unwrap();
        let path = format!("{};{system}\\System32", prefix.join(";"));
        let npm = std::process::Command::new(format!("{system}\\System32\\cmd.exe"))
            .args(["/d", "/c", "npm", "--version"])
            .current_dir(&project_dir)
            .env_clear()
            .env("PATH", &path)
            .env("SystemRoot", &system)
            .env("APPDATA", dir.path().join("appdata"))
            .env("LOCALAPPDATA", dir.path().join("localappdata"))
            .output()
            .unwrap();
        assert!(
            npm.status.success(),
            "{}",
            String::from_utf8_lossy(&npm.stderr)
        );
        assert!(
            String::from_utf8_lossy(&npm.stdout)
                .trim()
                .starts_with(char::is_numeric)
        );

        // A venv made by the installed Python is recognised for the project terminal.
        let venv = std::process::Command::new(tools.join("python.exe"))
            .args(["-I", "-m", "venv", ".venv", "--without-pip"])
            .current_dir(&project_dir)
            .output()
            .unwrap();
        assert!(
            venv.status.success(),
            "{}",
            String::from_utf8_lossy(&venv.stderr)
        );
        let scripts = crate::services::projects::detect(&project_dir)
            .virtual_env_scripts
            .unwrap();
        assert_eq!(
            Path::new(&scripts),
            project_dir.join(".venv").join("Scripts")
        );
    }

    /// Cross-cutting: disk full. Windows reports ERROR_DISK_FULL (112) / ERROR_HANDLE_DISK_FULL
    /// (39); both become DISK_SPACE_LOW with a plain message, never a generic failure.
    #[test]
    fn disk_full_is_reported_as_low_disk_space() {
        for code in [112, 39] {
            match io_failure("extracting", std::io::Error::from_raw_os_error(code)) {
                Outcome::Failed(f) => {
                    assert_eq!(f.code, ErrorCode::DiskSpaceLow);
                    assert!(f.message.contains("Ran out of disk space while extracting"));
                }
                _ => panic!("expected a failure"),
            }
        }
        match io_failure("extracting", std::io::Error::from_raw_os_error(5)) {
            Outcome::Failed(f) => assert_eq!(f.code, ErrorCode::Internal),
            _ => panic!("expected a failure"),
        }
        // The preflight asks for room to download, extract and swap.
        assert_eq!(disk_needed(100 * 1_048_576), 400 * 1_048_576);
        assert_eq!(disk_needed(u64::MAX), u64::MAX, "no overflow");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn interrupted_install_is_resumed_without_duplicates() {
        let h = harness(tool_zip(), |_| {}, "fixture-tool 1.2.3");
        // Simulate a crash mid-install: an `installing` row plus a half-written folder.
        let key = version_row_id("tool", "1.2.3", Architecture::X64);
        let pkg = h.ctx.catalog.packages[0].clone();
        let path = final_dir(&h).to_string_lossy().into_owned();
        h.ctx
            .db
            .write(|c| {
                persist_catalog_rows(c, "test", &pkg, &pkg.versions[0])?;
                installations::begin(c, &key, &path)
            })
            .unwrap();
        std::fs::create_dir_all(final_dir(&h)).unwrap();
        std::fs::write(final_dir(&h).join("partial.tmp"), b"half").unwrap();
        crate::services::recovery::reconcile(&h.ctx.db).unwrap();
        assert_eq!(installations(&h)[0].status, InstallStatus::Broken);

        assert_eq!(install(&h).await.status, OperationStatus::Succeeded);
        let inst = installations(&h);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].status, InstallStatus::Installed);
        assert!(
            !final_dir(&h).join("partial.tmp").exists(),
            "leftovers replaced by verified files"
        );
    }
}
