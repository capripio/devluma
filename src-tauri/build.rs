fn main() {
    // Embed the app manifest via the linker so test binaries get it too (tauri-build's
    // default only covers the main binary, which makes `cargo test` crash on Windows).
    let mut windows = tauri_build::WindowsAttributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    // Declaring app commands generates `allow-<command>` permissions, so each command
    // must be granted explicitly in `capabilities/` (ADR 0002 security baseline).
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(windows)
            .app_manifest(tauri_build::AppManifest::new().commands(&[
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
                "get_preferences",
                "set_preferences",
                "set_launch_at_login",
                "open_terminal",
                "get_storage_usage",
                "open_storage_folder",
                "list_scheduled_jobs",
                "preview_scheduled_job",
                "save_scheduled_job",
                "set_scheduled_job_enabled",
                "run_scheduled_job_now",
                "stop_scheduled_job_run",
                "delete_scheduled_job",
                "list_scheduled_job_runs",
                "get_scheduled_job_run_log",
            ])),
    )
    .expect("failed to run tauri-build");
}
