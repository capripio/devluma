pub mod commands;
pub mod domain;
pub mod error;
pub mod persistence;
pub mod platform;
pub mod security;
pub mod services;
pub mod state;

use std::path::PathBuf;

use tauri::Manager;

/// Builds the app and opens the metadata database before any IPC is served. Tests pass
/// `tauri::test::mock_builder()` and a temporary `data_dir`; production uses
/// `%LOCALAPPDATA%\Devluma` (ADR 0005).
fn build_app<R: tauri::Runtime>(
    base: tauri::Builder<R>,
    context: tauri::Context<R>,
    data_dir: Option<PathBuf>,
) -> tauri::Result<tauri::App<R>> {
    let app = base
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::app::get_startup_status,
            commands::operations::list_operations,
            commands::operations::cancel_operation,
            commands::packages::list_packages,
            commands::packages::install_package,
            commands::runtimes::preview_runtime_switch,
            commands::runtimes::apply_runtime_switch,
            commands::runtimes::get_php_details,
            commands::runtimes::create_php_ini,
            commands::projects::list_projects,
            commands::projects::create_project,
            commands::projects::remove_project,
            commands::projects::open_project_terminal,
            commands::services::list_services,
            commands::services::create_service,
            commands::services::start_service,
            commands::services::stop_service,
            commands::services::restart_service,
            commands::services::get_service_details,
            commands::services::remove_service,
            commands::services::open_service_ui,
            commands::setup::get_setup_status,
            commands::setup::choose_storage,
            commands::setup::run_setup,
            commands::setup::complete_setup,
            commands::setup::restart_app,
            commands::setup::reopen_setup,
            commands::updates::get_update_settings,
            commands::updates::set_update_urls,
            commands::updates::check_catalog_update,
            commands::updates::check_app_update,
            commands::updates::install_app_update,
            commands::diagnostics::preview_diagnostics,
            commands::diagnostics::export_diagnostics,
            commands::diagnostics::open_diagnostics_folder,
            commands::packages::preview_uninstall,
            commands::packages::uninstall_package,
            commands::services::set_service_ports,
            commands::databases::list_database_services,
            commands::databases::list_databases,
            commands::databases::list_database_backups,
            commands::databases::backup_database,
            commands::databases::snapshot_database,
            commands::databases::preview_restore,
            commands::databases::restore_database,
            commands::tunnels::get_ngrok_status,
            commands::tunnels::set_ngrok_path,
            commands::tunnels::set_ngrok_token,
            commands::tunnels::remove_ngrok_token,
            commands::tunnels::list_tunnels,
            commands::tunnels::start_tunnel,
            commands::tunnels::stop_tunnel,
            commands::sites::list_sites,
            commands::sites::preview_site,
            commands::sites::apply_site,
            commands::sites::remove_site,
            commands::sites::check_site,
            commands::sites::open_site,
            commands::sites::get_hosts,
            commands::sites::preview_hosts_change,
            commands::sites::apply_hosts_change,
            commands::certificates::list_certificates,
            commands::certificates::create_local_ca,
            commands::certificates::create_dev_certificate,
            commands::certificates::trust_certificate,
            commands::certificates::remove_certificate_trust,
            commands::certificates::renew_certificate,
            commands::certificates::delete_certificate,
            commands::settings::get_preferences,
            commands::settings::set_preferences,
            commands::settings::set_launch_at_login,
            commands::settings::open_terminal,
            commands::settings::get_storage_usage,
            commands::settings::open_storage_folder,
            commands::scheduler::list_scheduled_jobs,
            commands::scheduler::preview_scheduled_job,
            commands::scheduler::save_scheduled_job,
            commands::scheduler::set_scheduled_job_enabled,
            commands::scheduler::run_scheduled_job_now,
            commands::scheduler::stop_scheduled_job_run,
            commands::scheduler::delete_scheduled_job,
            commands::scheduler::list_scheduled_job_runs,
            commands::scheduler::get_scheduled_job_run_log,
        ])
        .build(context)?;
    let dir = match data_dir {
        Some(dir) => dir,
        None => services::storage::resolve(cfg!(debug_assertions)).root,
    };
    let sink: std::sync::Arc<dyn services::journal::ProgressSink> =
        std::sync::Arc::new(app.handle().clone());
    let states: std::sync::Arc<dyn services::supervisor::StateSink> =
        std::sync::Arc::new(app.handle().clone());
    let jobs: std::sync::Arc<dyn services::scheduler::SchedulerSink> =
        std::sync::Arc::new(app.handle().clone());
    let state = state::initialize(&dir, sink, states, jobs);
    if let Some(sup) = &state.supervisor {
        // Re-adopt services still running from a previous session, then keep checking health.
        sup.reconcile();
        sup.spawn_health_loop(std::time::Duration::from_secs(10));
        // Settings → Startup: services chosen to start with Devluma. Each start is a
        // journaled operation; ones already running (re-adopted above) are skipped.
        let start: Vec<String> = state
            .db()
            .ok()
            .and_then(|db| db.read(services::preferences::load).ok())
            .map(|p| p.start_services)
            .unwrap_or_default();
        for id in start {
            let _ = sup.start(&id);
        }
    }
    // Scheduled jobs (ADR 0017) run while Devluma is open.
    if let Some(s) = &state.scheduler {
        s.spawn_loop();
    }
    app.manage(state);
    Ok(app)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let storage = services::storage::resolve(cfg!(debug_assertions));
    let tmp = storage.root.join("tmp");
    let webview = storage.root.join("webview");
    let _ = std::fs::create_dir_all(&tmp);
    // SAFETY: set before Tauri, the runtime or any other thread starts (single-threaded here).
    // Child processes then keep their temp files in Devluma's folder (ADR 0014).
    unsafe {
        std::env::set_var("TEMP", &tmp);
        std::env::set_var("TMP", &tmp);
    }
    let app = build_app(
        tauri::Builder::default(),
        tauri::generate_context!(),
        Some(storage.root),
    )
    .expect("error while building Devluma");
    // The main window is created here rather than from tauri.conf.json (`create: false`) so
    // WebView2's data goes into Devluma's folder; Tauri would otherwise use
    // %LOCALAPPDATA%\<identifier> and ignore WEBVIEW2_USER_DATA_FOLDER (ADR 0014).
    let main = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .expect("main window config");
    let window = tauri::WebviewWindowBuilder::from_config(app.handle(), &main)
        .and_then(|b| b.data_directory(webview).build())
        .expect("error creating the main window");
    // Started by "Start with Windows": stay out of the way until the user opens it.
    if std::env::args().any(|a| a == platform::windows::autostart::AUTOSTART_ARG) {
        let _ = window.minimize();
    }
    app.run(|app, event| {
        // ADR 0006 default: Devluma stops the services it owns when it exits.
        // Tunnels always stop (their jobs would also end when Devluma's handles close).
        if let tauri::RunEvent::Exit = event {
            let state = app.state::<state::AppState>();
            if let Some(t) = &state.tunnels {
                t.stop_all();
            }
            if let Some(s) = &state.scheduler {
                s.stop_all();
            }
            if let Some(sup) = &state.supervisor {
                sup.stop_all_blocking();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
    use tauri::webview::InvokeRequest;
    use tauri::{App, WebviewWindow, WebviewWindowBuilder};

    // Uses the real tauri.conf.json and capabilities, so these tests exercise the ACL.
    fn app(data_dir: &std::path::Path) -> App<MockRuntime> {
        super::build_app(
            mock_builder(),
            tauri::generate_context!(),
            Some(data_dir.to_path_buf()),
        )
        .unwrap()
    }

    fn window(app: &App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
        WebviewWindowBuilder::new(app, label, Default::default())
            .build()
            .unwrap()
    }

    fn request(cmd: &str) -> InvokeRequest {
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        }
    }

    fn call(app: &App<MockRuntime>, cmd: &str) -> serde_json::Value {
        get_ipc_response(&window(app, "main"), request(cmd))
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
            .deserialize()
            .unwrap()
    }

    #[test]
    fn get_app_info_is_invokable_from_main_window() {
        let dir = tempfile::tempdir().unwrap();
        let info = call(&app(dir.path()), "get_app_info");
        assert_eq!(info["arch"], std::env::consts::ARCH);
        assert!(info["version"].is_string());
    }

    #[test]
    fn commands_are_denied_to_windows_without_a_capability() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        for cmd in [
            "get_app_info",
            "get_startup_status",
            "list_operations",
            "cancel_operation",
            "list_packages",
            "install_package",
            "preview_runtime_switch",
            "apply_runtime_switch",
            "get_php_details",
            "create_php_ini",
            "list_projects",
            "create_project",
            "remove_project",
            "open_project_terminal",
            "list_services",
            "create_service",
            "start_service",
            "stop_service",
            "restart_service",
            "get_service_details",
            "remove_service",
            "open_service_ui",
            "get_setup_status",
            "choose_storage",
            "run_setup",
            "complete_setup",
            "restart_app",
            "reopen_setup",
            "get_update_settings",
            "set_update_urls",
            "check_catalog_update",
            "check_app_update",
            "install_app_update",
            "preview_diagnostics",
            "export_diagnostics",
            "open_diagnostics_folder",
            "preview_uninstall",
            "uninstall_package",
            "set_service_ports",
            "list_database_services",
            "list_databases",
            "list_database_backups",
            "backup_database",
            "snapshot_database",
            "preview_restore",
            "restore_database",
            "get_ngrok_status",
            "set_ngrok_path",
            "set_ngrok_token",
            "remove_ngrok_token",
            "list_tunnels",
            "start_tunnel",
            "stop_tunnel",
            "list_sites",
            "preview_site",
            "apply_site",
            "remove_site",
            "check_site",
            "open_site",
            "get_hosts",
            "preview_hosts_change",
            "apply_hosts_change",
            "list_certificates",
            "create_local_ca",
            "create_dev_certificate",
            "trust_certificate",
            "remove_certificate_trust",
            "renew_certificate",
            "delete_certificate",
        ] {
            let res = get_ipc_response(&window(&app, &format!("untrusted-{cmd}")), request(cmd));
            assert!(res.is_err(), "{cmd}: expected ACL denial, got {res:?}");
        }
    }

    #[test]
    fn startup_creates_database_in_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let status = call(&app(dir.path()), "get_startup_status");
        assert_eq!(status["state"], "ready");
        assert!(dir.path().join("app.db").exists());
        assert_eq!(status["recovery"]["operationsNeedingRecovery"], 0);
    }

    #[test]
    fn list_packages_reports_the_bundled_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let view = call(&app(dir.path()), "list_packages");
        assert!(view["packages"].is_array());
        assert_eq!(view["issues"], serde_json::json!([]));
    }

    #[test]
    fn list_operations_returns_journal_rows() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let ops = call(&app, "list_operations");
        assert_eq!(ops, serde_json::json!([]));
    }
}
