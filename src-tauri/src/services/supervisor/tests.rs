//! Supervisor tests with real processes: the "fixture" service is this test binary re-run
//! as `fake_service`, which listens on a port, spawns a grandchild, and exits on a named
//! event — or crashes / ignores stop requests, depending on its mode.

use std::net::{TcpListener, TcpStream};
use std::sync::Mutex as StdMutex;

use super::behavior::tests_support::set_mode;
use super::*;
use crate::domain::catalog::{
    Architecture, ValidationOptions, fixtures, parse_and_validate, version_row_id,
};
use crate::domain::operation::OperationStatus;
use crate::persistence::operations as ops;
use crate::services::journal::test_support::RecordingSink;

// ---- the fake service process -----------------------------------------------------------

#[test]
#[ignore = "fake service process for supervisor tests"]
fn fake_service() {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    let Some(port) = std::env::var("DEVLUMA_FAKE_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
    else {
        return;
    };
    crate::platform::windows::job::tests::exit_with_parent();
    let mode = std::env::var("DEVLUMA_FAKE_MODE").unwrap_or_else(|_| "serve".into());
    println!("fake service starting in mode {mode} on port {port}");
    match mode.as_str() {
        "crash_on_start" => {
            eprintln!("fatal: could not open data file (password=hunter22)");
            std::process::exit(3);
        }
        "never_ready" => {
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        _ => {}
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    std::thread::spawn(move || for _ in listener.incoming() {});
    // A grandchild, to prove the whole tree is owned.
    let _grandchild = crate::platform::windows::job::tests::helper_command(
        "platform::windows::job::tests::sleep_helper",
    )
    .spawn();
    match mode.as_str() {
        "crash_later" => {
            std::thread::sleep(Duration::from_millis(800));
            std::process::exit(4);
        }
        "ignore_stop" => std::thread::sleep(Duration::from_secs(120)),
        _ => {
            let name: Vec<u16> =
                std::ffi::OsStr::new(&format!("DevlumaFakeStop{}", std::process::id()))
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
            // SAFETY: NUL-terminated name; the event lives until the process exits.
            unsafe {
                let ev =
                    CreateEventW(None, true, false, windows::core::PCWSTR(name.as_ptr())).unwrap();
                WaitForSingleObject(ev, u32::MAX);
            }
            println!("fake service shutting down cleanly");
        }
    }
}

// ---- harness ------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct RecordingStates(StdMutex<Vec<ServiceStatus>>);

impl StateSink for RecordingStates {
    fn emit_state(&self, s: &ServiceStatus) {
        self.0.lock().unwrap().push(s.clone());
    }
}

struct World {
    _dir: tempfile::TempDir,
    db: Arc<Database>,
    sup: Arc<Supervisor>,
    paths: DataPaths,
    catalog: Arc<Catalog>,
    installation_id: String,
    states: Arc<RecordingStates>,
    /// Every job name seen, so cleanup works even after the DB forgot it.
    jobs: StdMutex<Vec<String>>,
}

impl Drop for World {
    /// Never leak fake processes, even when an assertion failed.
    fn drop(&mut self) {
        self.remember_jobs();
        for name in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if let Some(job) = job::Job::open(name) {
                let _ = job.terminate(1);
                job.wait_empty(Duration::from_secs(5));
            }
        }
    }
}

pub(crate) fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn world() -> World {
    let catalog = Arc::new(
        parse_and_validate(
            &fixtures::document(vec![fixtures::package(
                "svc",
                vec![fixtures::version("1.0.0")],
            )]),
            ValidationOptions::default(),
        )
        .unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths::under(dir.path());
    paths.create_all().unwrap();
    let db = Arc::new(crate::persistence::test_support::db());
    let key = version_row_id("svc", "1.0.0", Architecture::X64);
    let installation_id = db
        .write(|c| {
            crate::persistence::packages::upsert(
                c,
                &crate::persistence::packages::fixtures::package("svc"),
            )?;
            crate::persistence::packages::insert_version(
                c,
                &crate::persistence::packages::fixtures::version(&key, "svc", "1.0.0"),
            )?;
            let i = installations::begin(c, &key, &dir.path().join("pkg").to_string_lossy())?;
            installations::mark_installed(c, &i.id, "ok").map(|i| i.id)
        })
        .unwrap();
    let states = Arc::new(RecordingStates::default());
    let sup = Supervisor::new(
        Arc::clone(&db),
        Arc::clone(&catalog),
        paths.clone(),
        Arc::new(RecordingSink::default()),
        states.clone(),
    );
    World {
        _dir: dir,
        db,
        sup,
        paths,
        catalog,
        installation_id,
        states,
        jobs: StdMutex::new(Vec::new()),
    }
}

impl World {
    fn create(&self, name: &str, port: u16, mode: &str) -> String {
        let (op_id, handle) = self
            .sup
            .create(CreateServiceRequest {
                installation_id: self.installation_id.clone(),
                name: name.into(),
                port,
                admin_port: None,
                data_path: None,
                use_existing_data: false,
            })
            .unwrap();
        block(handle);
        assert_eq!(
            self.op(&op_id).status,
            OperationStatus::Succeeded,
            "{:?}",
            self.op(&op_id).safe_error_message
        );
        let id = self
            .db
            .read(services::list)
            .unwrap()
            .into_iter()
            .find(|s| s.instance_name == name)
            .unwrap()
            .id;
        set_mode(&id, mode);
        id
    }

    fn remember_jobs(&self) {
        let names: Vec<String> = self
            .db
            .read(services::list)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| s.job_name)
            .collect();
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for n in names {
            if !jobs.contains(&n) {
                jobs.push(n);
            }
        }
    }

    fn op(&self, id: &str) -> ops::Operation {
        self.db.read(|c| ops::get(c, id)).unwrap()
    }

    fn svc(&self, id: &str) -> ServiceInstance {
        self.db.read(|c| services::get(c, id)).unwrap()
    }

    fn run(&self, started: Result<Started, IpcError>) -> ops::Operation {
        let (op, handle) = started.unwrap();
        block(handle);
        self.remember_jobs();
        self.op(&op)
    }

    /// Processes in any job this service has used (job names embed the service id).
    fn job_pids(&self, id: &str) -> Vec<u32> {
        self.remember_jobs();
        let names = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        names
            .iter()
            .filter(|n| n.contains(id))
            .filter_map(|n| job::Job::open(n))
            .flat_map(|j| j.process_ids().unwrap_or_default())
            .collect()
    }

    fn wait_for_state(&self, id: &str, state: ServiceState) -> bool {
        for _ in 0..200 {
            if self.svc(id).observed_state == state {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

fn block(h: tauri::async_runtime::JoinHandle<()>) {
    tauri::async_runtime::block_on(h).unwrap();
}

// ---- tests --------------------------------------------------------------------------------

#[test]
fn starts_stops_and_owns_the_whole_process_tree() {
    let w = world();
    let port = free_port();
    let id = w.create("web", port, "serve");
    assert_eq!(w.svc(&id).observed_state, ServiceState::Stopped);

    let op = w.run(w.sup.start(&id));
    assert_eq!(
        op.status,
        OperationStatus::Succeeded,
        "{:?}",
        op.safe_error_message
    );
    let s = w.svc(&id);
    assert_eq!(s.observed_state, ServiceState::Running);
    assert_eq!(s.health_status, "healthy");
    assert!(s.pid.is_some() && s.process_started_at.is_some());
    assert!(ports::accepts_connections(port, Duration::from_secs(1)));
    for _ in 0..100 {
        if w.job_pids(&id).len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(
        w.job_pids(&id).len() >= 2,
        "service and its child are both owned: pids={:?} job={:?} known={:?} log={}",
        w.job_pids(&id),
        w.svc(&id).job_name,
        w.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        std::fs::read_to_string(w.paths.services.join(&id).join("logs").join("service.log"))
            .unwrap_or_default()
    );

    let op = w.run(w.sup.stop(&id));
    assert_eq!(
        op.status,
        OperationStatus::Succeeded,
        "{:?}",
        op.safe_error_message
    );
    assert_eq!(w.svc(&id).observed_state, ServiceState::Stopped);
    assert!(w.svc(&id).pid.is_none());
    assert!(w.job_pids(&id).is_empty(), "no owned process survives stop");
    let log = std::fs::read_to_string(w.paths.services.join(&id).join("logs").join("service.log"))
        .unwrap();
    assert!(
        log.contains("shutting down cleanly"),
        "graceful path was used: {log}"
    );

    let seen: Vec<ServiceState> = w
        .states
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.observed_state)
        .collect();
    for expected in [
        ServiceState::Starting,
        ServiceState::Running,
        ServiceState::Stopping,
        ServiceState::Stopped,
    ] {
        assert!(seen.contains(&expected), "missing {expected:?} in {seen:?}");
    }
}

#[test]
fn port_conflict_is_reported_and_the_other_process_is_left_alone() {
    let w = world();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = other.local_addr().unwrap().port();
    let id = w.create("web", port, "serve");

    let op = w.run(w.sup.start(&id));
    assert_eq!(op.status, OperationStatus::Failed);
    assert_eq!(op.error_code.as_deref(), Some("PORT_IN_USE"));
    let msg = op.safe_error_message.unwrap();
    assert!(
        msg.contains(&format!("Port {port}"))
            && msg.contains(&format!("PID {}", std::process::id())),
        "{msg}"
    );
    assert_eq!(
        w.svc(&id).observed_state,
        ServiceState::Stopped,
        "nothing was started"
    );
    assert!(w.job_pids(&id).is_empty());
    // The unrelated listener (this test process) is untouched.
    assert!(ports::accepts_connections(port, Duration::from_secs(1)));
    drop(other);
}

#[test]
fn invalid_config_never_starts_a_process() {
    let w = world();
    let id = w.create("web", free_port(), "invalid_config");
    let op = w.run(w.sup.start(&id));
    assert_eq!(op.error_code.as_deref(), Some("CONFIG_INVALID"));
    assert_eq!(w.svc(&id).observed_state, ServiceState::Stopped);
    assert_eq!(
        w.svc(&id).last_error_code.as_deref(),
        Some("CONFIG_INVALID")
    );
    assert!(w.job_pids(&id).is_empty());
}

#[test]
fn crash_during_startup_is_failed_with_exit_code_and_redacted_log() {
    let w = world();
    let id = w.create("web", free_port(), "crash_on_start");
    let op = w.run(w.sup.start(&id));
    assert_eq!(op.status, OperationStatus::Failed);
    assert_eq!(op.error_code.as_deref(), Some("PROCESS_EXITED"));
    let msg = op.safe_error_message.unwrap();
    assert!(msg.contains("exit code 3"), "{msg}");
    assert!(!msg.contains("hunter22"), "log excerpt is redacted: {msg}");
    let s = w.svc(&id);
    assert_eq!(
        (s.observed_state, s.last_exit_code),
        (ServiceState::Failed, Some(3))
    );

    let details = w.sup.details(&id).unwrap();
    let log = details.logs[0].lines.join("\n");
    assert!(
        log.contains("password=[REDACTED]") && !log.contains("hunter22"),
        "{log}"
    );
}

#[test]
fn never_ready_service_is_stopped_after_the_timeout() {
    let w = world();
    let id = w.create("web", free_port(), "never_ready");
    let op = w.run(w.sup.start(&id));
    assert_eq!(op.error_code.as_deref(), Some("PROCESS_EXITED"));
    assert!(
        op.safe_error_message
            .unwrap()
            .contains("didn't start accepting connections")
    );
    assert!(
        w.job_pids(&id).is_empty(),
        "the stuck process was cleaned up"
    );
}

#[test]
fn unexpected_exit_after_start_marks_the_service_failed() {
    let w = world();
    let id = w.create("web", free_port(), "crash_later");
    assert_eq!(w.run(w.sup.start(&id)).status, OperationStatus::Succeeded);
    assert!(
        w.wait_for_state(&id, ServiceState::Failed),
        "monitor noticed the exit"
    );
    let s = w.svc(&id);
    assert_eq!(s.last_exit_code, Some(4));
    assert!(s.pid.is_none());
    assert!(
        w.job_pids(&id).is_empty(),
        "the orphaned grandchild was cleaned up too"
    );
    let crash_ops = w.db.read(|c| ops::list_for_display(c, 50)).unwrap();
    assert!(
        crash_ops
            .iter()
            .any(|o| o.operation_type == "service_exited"),
        "journaled for the tray"
    );
}

#[test]
fn ignored_graceful_stop_falls_back_to_terminating_the_owned_tree() {
    let w = world();
    let id = w.create("web", free_port(), "ignore_stop");
    assert_eq!(w.run(w.sup.start(&id)).status, OperationStatus::Succeeded);
    // Make the grace period irrelevant: the fake never creates its stop event.
    let op = w.run(w.sup.stop(&id));
    assert_eq!(op.status, OperationStatus::Succeeded);
    assert_eq!(w.svc(&id).observed_state, ServiceState::Stopped);
    assert!(w.job_pids(&id).is_empty());
}

#[test]
fn running_service_is_re_adopted_after_a_restart_only_with_proof() {
    let w = world();
    let port = free_port();
    let id = w.create("web", port, "serve");
    assert_eq!(w.run(w.sup.start(&id)).status, OperationStatus::Succeeded);
    let pid = w.svc(&id).pid.unwrap();

    // Simulate a Devluma restart: startup recovery marks it unknown, a fresh supervisor reconciles.
    w.db.write(|c| services::mark_all_unknown(c)).unwrap();
    let fresh = Supervisor::new(
        Arc::clone(&w.db),
        Arc::clone(&w.catalog),
        w.paths.clone(),
        Arc::new(RecordingSink::default()),
        Arc::new(RecordingStates::default()),
    );
    fresh.reconcile();
    assert_eq!(w.svc(&id).observed_state, ServiceState::Running, "adopted");
    assert_eq!(w.svc(&id).pid, Some(pid));
    assert_eq!(w.run(fresh.stop(&id)).status, OperationStatus::Succeeded);
    assert!(w.job_pids(&id).is_empty());

    // Now with a mismatched creation time (as if the PID had been reused): not adopted, not killed.
    let op = w.run(fresh.start(&id));
    assert_eq!(
        op.status,
        OperationStatus::Succeeded,
        "{:?}",
        op.safe_error_message
    );
    let pid = w.svc(&id).pid.unwrap();
    w.db.write(|c| {
        let job = services::get(c, &id)?.job_name;
        services::set_process(c, &id, Some(pid as u32), Some("12345"), job.as_deref())?;
        services::mark_all_unknown(c)
    })
    .unwrap();
    let another = Supervisor::new(
        Arc::clone(&w.db),
        Arc::clone(&w.catalog),
        w.paths.clone(),
        Arc::new(RecordingSink::default()),
        Arc::new(RecordingStates::default()),
    );
    another.reconcile();
    assert_eq!(
        w.svc(&id).observed_state,
        ServiceState::Stopped,
        "unproven process isn't adopted"
    );
    assert!(
        job::identity(pid as u32).is_some(),
        "…and it isn't killed either"
    );
    // (World's Drop cleans it up.)
}

#[test]
fn concurrent_actions_on_one_service_are_refused() {
    let w = world();
    let id = w.create("web", free_port(), "serve");
    let first = w.sup.start(&id).unwrap();
    let second = w.sup.start(&id);
    assert!(
        second.is_err(),
        "second start while the first runs is refused"
    );
    block(first.1);
    assert_eq!(w.run(w.sup.stop(&id)).status, OperationStatus::Succeeded);
}

#[test]
fn creation_validates_ports_and_names() {
    let w = world();
    let port = free_port();
    w.create("web", port, "serve");
    let req = |name: &str, port: u16| CreateServiceRequest {
        installation_id: w.installation_id.clone(),
        name: name.into(),
        port,
        admin_port: None,
        data_path: None,
        use_existing_data: false,
    };
    assert!(
        w.sup
            .create(req("web2", port))
            .unwrap_err()
            .message
            .contains("already assigned")
    );
    assert!(w.sup.create(req("bad name!", free_port())).is_err());
    assert!(w.sup.create(req("x", 0)).is_err());
}

#[test]
fn start_all_starts_a_projects_services_and_skips_taken_ports() {
    use crate::persistence::projects::{self, ProjectFields};
    use crate::services::projects::start_all;
    let w = world();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = other.local_addr().unwrap().port();
    let web = w.create("web", taken, "serve");
    let db_svc = w.create("db", free_port(), "serve");
    let project = |name: &str, services: &[String]| {
        w.db.write(|c| {
            let p = projects::create(
                c,
                &ProjectFields {
                    name: name.into(),
                    root_path: format!(r"D:\code\{name}"),
                    ..Default::default()
                },
            )?;
            projects::set_service_ids(c, &p.id, services)?;
            Ok(p.id)
        })
        .unwrap()
    };
    let shop = project("shop", &[web.clone(), db_svc.clone()]);

    let r = start_all(&w.db, &w.sup, &shop).unwrap();
    assert_eq!(r.started.len(), 1, "{r:?}");
    assert_eq!(r.started[0].service_name, "db");
    // The service whose port another program holds is skipped, naming that program.
    assert_eq!(r.not_started.len(), 1, "{r:?}");
    let msg = &r.not_started[0].message;
    assert!(
        msg.contains(&format!("Port {taken}"))
            && msg.contains(&format!("PID {}", std::process::id())),
        "{msg}"
    );
    assert_eq!(w.svc(&web).observed_state, ServiceState::Stopped);
    assert!(w.wait_for_state(&db_svc, ServiceState::Running));
    w.remember_jobs();

    // Running services are reported, not started again.
    let again = start_all(&w.db, &w.sup, &shop).unwrap();
    assert_eq!(again.already_running, ["db"]);
    assert!(again.started.is_empty());

    // A site's web server counts even when it wasn't chosen; no services at all is an error.
    let blog = project("blog", &[]);
    let site = crate::persistence::sites::test_support::insert(&w.db, &blog, "blog.test", 80);
    w.db.write(|c| {
        Ok(c.execute(
            "UPDATE sites SET web_server_service_id = ?1 WHERE id = ?2",
            [&db_svc, &site],
        )?)
    })
    .unwrap();
    let ids: Vec<String> =
        w.db.read(|c| crate::services::projects::services_to_start(c, &blog))
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
    assert_eq!(ids, std::slice::from_ref(&db_svc));
    let empty = project("empty", &[]);
    assert!(start_all(&w.db, &w.sup, &empty).is_err());

    assert_eq!(
        w.run(w.sup.stop(&db_svc)).status,
        OperationStatus::Succeeded
    );
    drop(other);
}

#[test]
fn ports_change_only_while_stopped_and_never_collide() {
    let w = world();
    let (p1, p2, p3) = (free_port(), free_port(), free_port());
    let a = w.create("a", p1, "serve");
    w.create("b", p2, "serve");
    assert!(
        w.sup
            .set_ports(&a, p2, None)
            .unwrap_err()
            .message
            .contains("already assigned")
    );
    assert!(w.sup.set_ports(&a, 0, None).is_err());
    assert_eq!(
        w.sup.set_ports(&a, p3, None).unwrap().port,
        Some(i64::from(p3))
    );

    assert_eq!(w.run(w.sup.start(&a)).status, OperationStatus::Succeeded);
    assert!(
        TcpStream::connect(("127.0.0.1", p3)).is_ok(),
        "started on the new port"
    );
    assert!(
        w.sup
            .set_ports(&a, p1, None)
            .unwrap_err()
            .message
            .contains("Stop the service")
    );
    assert_eq!(w.run(w.sup.stop(&a)).status, OperationStatus::Succeeded);
}

#[test]
fn removing_a_service_keeps_user_data_and_refuses_while_running() {
    let w = world();
    let id = w.create("web", free_port(), "serve");
    assert_eq!(w.run(w.sup.start(&id)).status, OperationStatus::Succeeded);
    assert!(w.sup.remove(&id).is_err(), "must stop first");
    assert_eq!(w.run(w.sup.stop(&id)).status, OperationStatus::Succeeded);

    let data = w.paths.data.join("keep-me");
    std::fs::create_dir_all(&data).unwrap();
    w.db.write(|c| services::set_paths(c, &id, None, Some(&data.to_string_lossy())))
        .unwrap();
    assert_eq!(
        w.sup.remove(&id).map_err(|e| e.message).unwrap(),
        Some(data.to_string_lossy().into_owned())
    );
    assert!(data.exists(), "data folder untouched");
    assert!(
        !w.paths.services.join(&id).exists(),
        "managed folder removed"
    );
}

/// Real end-to-end: install Nginx and MariaDB from the bundled catalog (official downloads,
/// ~100 MB), create services, start, talk to them, stop, and prove nothing is left running.
/// `cargo test live_nginx_and_mariadb -- --ignored`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and real services"]
async fn live_nginx_and_mariadb_services() {
    use crate::services::download::DownloadOptions;
    use crate::services::install::{InstallContext, InstallManager, InstallRequest, ProcessProbe};

    let catalog = Arc::new(crate::services::catalog::load());
    assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths::under(dir.path());
    paths.create_all().unwrap();
    let db = Arc::new(crate::persistence::test_support::db());
    let ctx = InstallContext {
        db: Arc::clone(&db),
        catalog: Arc::clone(&catalog),
        paths: paths.clone(),
        probe: Arc::new(ProcessProbe),
        sink: Arc::new(RecordingSink::default()),
        download: DownloadOptions::default(),
    };
    let manager = Arc::new(InstallManager::default());
    for (pkg, version) in [("nginx", "1.30.5"), ("mariadb", "11.8.9")] {
        let (started, handle) = manager
            .start(
                ctx.clone(),
                InstallRequest {
                    package_id: pkg.into(),
                    version: version.into(),
                    architecture: Architecture::X64,
                },
            )
            .unwrap();
        handle.await.unwrap();
        let op = db.read(|c| ops::get(c, &started.operation_id)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{pkg}: {:?}",
            op.safe_error_message
        );
    }
    let installed = db.read(installations::list).unwrap();
    let inst = |pkg: &str| {
        installed
            .iter()
            .find(|i| i.package_id == pkg)
            .unwrap()
            .id
            .clone()
    };

    let sup = Supervisor::new(
        Arc::clone(&db),
        Arc::clone(&catalog),
        paths.clone(),
        Arc::new(RecordingSink::default()),
        Arc::new(RecordingStates::default()),
    );
    let _cleanup = super::StopAllOnDrop(Arc::clone(&sup));
    let (web_port, db_port) = (free_port(), free_port());
    for (pkg, name, port) in [("nginx", "web", web_port), ("mariadb", "db", db_port)] {
        let (op, h) = sup
            .create(CreateServiceRequest {
                installation_id: inst(pkg),
                name: name.into(),
                port,
                admin_port: None,
                data_path: None,
                use_existing_data: false,
            })
            .unwrap();
        h.await.unwrap();
        let op = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{name}: {:?}",
            op.safe_error_message
        );
    }
    let all = db.read(services::list).unwrap();
    let id = |name: &str| {
        all.iter()
            .find(|s| s.instance_name == name)
            .unwrap()
            .id
            .clone()
    };
    let (web, dbs) = (id("web"), id("db"));

    for svc in [&web, &dbs] {
        let (op, h) = sup.start(svc).unwrap();
        h.await.unwrap();
        let op = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
    }

    // Nginx serves the managed welcome page on loopback.
    let body = reqwest::get(format!("http://127.0.0.1:{web_port}/"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("managed by Devluma"), "{body}");

    // MariaDB answers a query through the official client.
    let data_dir = paths.data.join("mariadb").join("db");
    assert!(data_dir.join("mysql").is_dir(), "data folder initialized");
    let client = std::path::PathBuf::from(
        &installed
            .iter()
            .find(|i| i.package_id == "mariadb")
            .unwrap()
            .install_path,
    )
    .join("bin")
    .join("mariadb.exe");
    let port_arg = format!("--port={db_port}");
    let out = crate::platform::windows::process::run_captured(
        &client,
        &[
            "--host=127.0.0.1",
            &port_arg,
            "--user=root",
            "-N",
            "-e",
            "select 40+2",
        ],
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(out.stdout.trim(), "42", "stderr: {}", out.stderr);

    for svc in [&web, &dbs] {
        let job_name = db
            .read(|c| services::get(c, svc))
            .unwrap()
            .job_name
            .unwrap();
        let (op, h) = sup.stop(svc).unwrap();
        h.await.unwrap();
        let op = db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            op.status,
            OperationStatus::Succeeded,
            "{:?}",
            op.safe_error_message
        );
        let left = job::Job::open(&job_name)
            .map(|j| j.process_ids().unwrap())
            .unwrap_or_default();
        assert!(left.is_empty(), "owned processes left: {left:?}");
        let events = db.read(|c| ops::list_events(c, &op.id)).unwrap();
        assert!(
            !events
                .iter()
                .any(|e| e.message.contains("Graceful shutdown didn't finish")),
            "real services stop gracefully"
        );
    }
    assert!(ports::is_free(web_port) && ports::is_free(db_port));
}

/// `DEVLUMA_LIVE_CACHE=<folder>`: archives already downloaded (and checked) from the real
/// sources are served from that folder instead of downloading them again. Everything after
/// the download is unchanged: SHA-256, size, extraction and the version probes still run on
/// the real bytes. Only files whose name matches a catalog URL are used.
pub(crate) fn local_cache(
    catalog: &mut crate::domain::catalog::Catalog,
) -> Option<crate::services::download::test_server::TestServer> {
    use crate::services::download::test_server::{Route, serve_owned};
    let dir = std::path::PathBuf::from(std::env::var_os("DEVLUMA_LIVE_CACHE")?);
    let mut routes = Vec::new();
    for pkg in &catalog.packages {
        for v in &pkg.versions {
            let name = v
                .source
                .url
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            if dir.join(&name).is_file() {
                routes.push((format!("/{name}"), Route::File(dir.join(&name))));
            }
        }
    }
    if routes.is_empty() {
        return None;
    }
    let server = serve_owned(routes);
    for pkg in &mut catalog.packages {
        for v in &mut pkg.versions {
            let name = v
                .source
                .url
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            if dir.join(&name).is_file() {
                eprintln!("live cache: serving {name} locally");
                v.source.url = format!("{}/{name}", server.base);
                pkg.manifest.allowed_hosts.push("127.0.0.1".into());
            }
        }
    }
    Some(server)
}

/// Shared setup for live service tests: real installs from the bundled catalog, then a
/// supervisor over them. Services are stopped when the env is dropped.
pub(crate) struct LiveEnv {
    pub _dir: tempfile::TempDir,
    pub db: Arc<crate::persistence::Database>,
    #[allow(dead_code)]
    pub paths: DataPaths,
    pub sup: Arc<Supervisor>,
    _cleanup: super::StopAllOnDrop,
}

impl LiveEnv {
    pub async fn install(packages: &[(&str, &str)]) -> Self {
        use crate::services::download::DownloadOptions;
        use crate::services::install::{
            InstallContext, InstallManager, InstallRequest, ProcessProbe,
        };

        let mut catalog = crate::services::catalog::load();
        assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
        let cache = local_cache(&mut catalog);
        let catalog = Arc::new(catalog);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::under(dir.path());
        paths.create_all().unwrap();
        let db = Arc::new(crate::persistence::test_support::db());
        let ctx = InstallContext {
            db: Arc::clone(&db),
            catalog: Arc::clone(&catalog),
            paths: paths.clone(),
            probe: Arc::new(ProcessProbe),
            sink: Arc::new(RecordingSink::default()),
            download: DownloadOptions {
                allow_loopback_http: cache.is_some(),
                ..DownloadOptions::default()
            },
        };
        let manager = Arc::new(InstallManager::default());
        for (pkg, version) in packages {
            let (started, handle) = manager
                .start(
                    ctx.clone(),
                    InstallRequest {
                        package_id: (*pkg).into(),
                        version: (*version).into(),
                        architecture: Architecture::X64,
                    },
                )
                .unwrap();
            handle.await.unwrap();
            let op = db.read(|c| ops::get(c, &started.operation_id)).unwrap();
            assert_eq!(
                op.status,
                OperationStatus::Succeeded,
                "{pkg}: {:?}",
                op.safe_error_message
            );
        }
        let sup = Supervisor::new(
            Arc::clone(&db),
            catalog,
            paths.clone(),
            Arc::new(RecordingSink::default()),
            Arc::new(RecordingStates::default()),
        );
        drop(cache);
        Self {
            _cleanup: super::StopAllOnDrop(Arc::clone(&sup)),
            _dir: dir,
            db,
            paths,
            sup,
        }
    }

    pub fn installation(&self, package: &str) -> crate::persistence::installations::Installation {
        self.db
            .read(installations::list)
            .unwrap()
            .into_iter()
            .find(|i| i.package_id == package)
            .unwrap()
    }

    async fn expect_ok(&self, op: String, h: tauri::async_runtime::JoinHandle<()>) -> String {
        h.await.unwrap();
        let o = self.db.read(|c| ops::get(c, &op)).unwrap();
        assert_eq!(
            o.status,
            OperationStatus::Succeeded,
            "{:?}",
            o.safe_error_message
        );
        op
    }

    /// Creates a service and returns its id.
    pub async fn create(
        &self,
        package: &str,
        name: &str,
        port: u16,
        admin_port: Option<u16>,
    ) -> String {
        let (op, h) = self
            .sup
            .create(CreateServiceRequest {
                installation_id: self.installation(package).id,
                name: name.into(),
                port,
                admin_port,
                data_path: None,
                use_existing_data: false,
            })
            .unwrap();
        self.expect_ok(op, h).await;
        self.db
            .read(services::list)
            .unwrap()
            .into_iter()
            .find(|s| s.instance_name == name)
            .unwrap()
            .id
    }

    pub async fn start(&self, id: &str) {
        let (op, h) = self.sup.start(id).unwrap();
        self.expect_ok(op, h).await;
    }

    /// Stops a service and checks it shut down cleanly with nothing left in its job.
    pub async fn stop_cleanly(&self, id: &str, expect_graceful: bool) {
        let job_name = self
            .db
            .read(|c| services::get(c, id))
            .unwrap()
            .job_name
            .unwrap();
        let (op, h) = self.sup.stop(id).unwrap();
        let op = self.expect_ok(op, h).await;
        let left = job::Job::open(&job_name)
            .map(|j| j.process_ids().unwrap())
            .unwrap_or_default();
        assert!(left.is_empty(), "owned processes left: {left:?}");
        if expect_graceful {
            let events = self.db.read(|c| ops::list_events(c, &op)).unwrap();
            assert!(
                !events
                    .iter()
                    .any(|e| e.message.contains("Graceful shutdown didn't finish")),
                "should stop gracefully"
            );
        }
    }
}

/// `cargo test live_mailpit -- --ignored`: real Mailpit catches a message sent over SMTP and
/// shows it through its API; both ports are loopback-only.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and real services"]
async fn live_mailpit_catches_mail() {
    use std::io::{BufRead, BufReader, Write};
    let env = LiveEnv::install(&[("mailpit", "1.31.5")]).await;
    let (smtp, ui) = (free_port(), free_port());
    let id = env.create("mailpit", "mail", smtp, Some(ui)).await;
    let row = env.db.read(|c| services::get(c, &id)).unwrap();
    assert_eq!(
        (row.service_type.as_str(), row.admin_port),
        ("mail", Some(i64::from(ui)))
    );
    env.start(&id).await;

    // A plain SMTP conversation, with AUTH like most frameworks send.
    let stream = std::net::TcpStream::connect(("127.0.0.1", smtp)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut w = stream;
    let mut expect = |code: &str| loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with(code), "expected {code}, got {line:?}");
        if line.as_bytes().get(3) == Some(&b' ') {
            break;
        }
    };
    expect("220");
    for (cmd, code) in [
        ("EHLO devluma.test", "250"),
        ("AUTH PLAIN AHVzZXIAc2VjcmV0", "235"),
        ("MAIL FROM:<app@shop.test>", "250"),
        ("RCPT TO:<customer@example.com>", "250"),
        ("DATA", "354"),
    ] {
        write!(w, "{cmd}\r\n").unwrap();
        expect(code);
    }
    write!(
        w,
        "Subject: Order shipped\r\n\r\nHello from Devluma.\r\n.\r\n"
    )
    .unwrap();
    expect("250");
    write!(w, "QUIT\r\n").unwrap();

    let list = reqwest::get(format!("http://127.0.0.1:{ui}/api/v1/messages"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let list: serde_json::Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list["total"], 1, "{list}");
    assert_eq!(list["messages"][0]["Subject"], "Order shipped");

    let details = env.sup.details(&id).unwrap();
    assert!(
        details
            .config_text
            .unwrap()
            .contains(&format!("127.0.0.1:{smtp}"))
    );
    env.stop_cleanly(&id, true).await;
    // Stopped means the ports are usable again (sockets released), so a restart works.
    assert!(ports::is_free(smtp) && ports::is_free(ui));
}

/// `cargo test live_postgresql -- --ignored`: initdb into a new data folder, then a real query
/// through psql; data survives a clean stop and restart. Set `DEVLUMA_LIVE_CACHE` to reuse
/// already-downloaded archives (PostgreSQL's is ~385 MB).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and real services"]
async fn live_postgresql_database() {
    let env = LiveEnv::install(&[("postgresql", "18.6")]).await;
    let port = free_port();
    let id = env.create("postgresql", "pg", port, None).await;
    let data = std::path::PathBuf::from(
        env.db
            .read(|c| services::get(c, &id))
            .unwrap()
            .data_path
            .unwrap(),
    );
    assert!(data.join("PG_VERSION").is_file(), "initialized by initdb");
    let psql = std::path::PathBuf::from(env.installation("postgresql").install_path)
        .join("bin")
        .join("psql.exe");
    let port_arg = port.to_string();
    let sql = |q: &str| {
        let out = crate::platform::windows::process::run_captured(
            &psql,
            &[
                "-h",
                "127.0.0.1",
                "-p",
                &port_arg,
                "-U",
                "postgres",
                "-tAc",
                q,
            ],
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(0), "{}", out.stderr);
        out.stdout.trim().to_owned()
    };

    env.start(&id).await;
    assert_eq!(sql("select 40+2"), "42");
    assert_eq!(sql("show listen_addresses"), "127.0.0.1");
    sql("create table notes (body text); insert into notes values ('kept')");
    env.stop_cleanly(&id, true).await;
    assert!(ports::is_free(port));

    env.start(&id).await;
    assert_eq!(sql("select body from notes"), "kept");
    env.stop_cleanly(&id, true).await;
}

/// Minimal MongoDB wire-protocol `ping` (OP_MSG); true if the server answers `ok: 1.0`.
pub(crate) fn mongo_ping(port: u16) -> bool {
    use std::io::{Read, Write};
    let mut doc = Vec::new();
    doc.extend([0x10]);
    doc.extend(b"ping\0");
    doc.extend(1i32.to_le_bytes());
    doc.extend([0x02]);
    doc.extend(b"$db\0");
    doc.extend(6i32.to_le_bytes());
    doc.extend(b"admin\0");
    doc.push(0);
    let mut bson = ((doc.len() + 4) as i32).to_le_bytes().to_vec();
    bson.extend(doc);
    let mut body = 0u32.to_le_bytes().to_vec(); // flag bits
    body.push(0); // section kind 0: body
    body.extend(bson);
    let mut msg = ((16 + body.len()) as i32).to_le_bytes().to_vec();
    msg.extend(1i32.to_le_bytes()); // request id
    msg.extend(0i32.to_le_bytes()); // response to
    msg.extend(2013i32.to_le_bytes()); // OP_MSG
    msg.extend(body);
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    if s.write_all(&msg).is_err() {
        return false;
    }
    let mut buf = vec![0u8; 4096];
    let n = s.read(&mut buf).unwrap_or(0);
    let ok_one: &[u8] = b"\x01ok\x00\x00\x00\x00\x00\x00\x00\xf0\x3f";
    buf[..n].windows(ok_one.len()).any(|w| w == ok_one)
}

/// `cargo test live_mongodb -- --ignored`: mongod on loopback answers a ping, shuts down
/// cleanly via its shutdown event and starts again on the same data. Set
/// `DEVLUMA_LIVE_CACHE` to reuse already-downloaded archives (MongoDB's is ~1 GB).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and real services"]
async fn live_mongodb_database() {
    let env = LiveEnv::install(&[("mongodb", "9.0.2")]).await;
    let port = free_port();
    let id = env.create("mongodb", "mongo", port, None).await;
    let data = std::path::PathBuf::from(
        env.db
            .read(|c| services::get(c, &id))
            .unwrap()
            .data_path
            .unwrap(),
    );

    env.start(&id).await;
    assert!(mongo_ping(port), "mongod answers ping");
    assert!(
        data.join("WiredTiger").is_file(),
        "data files created on first start"
    );
    let config = env.sup.details(&id).unwrap().config_text.unwrap();
    assert!(config.contains("bindIp: 127.0.0.1"), "{config}");
    env.stop_cleanly(&id, true).await;
    assert!(ports::is_free(port));

    env.start(&id).await;
    assert!(mongo_ping(port));
    env.stop_cleanly(&id, true).await;
}
