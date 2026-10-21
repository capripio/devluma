use std::sync::Mutex;

use chrono::TimeZone;

use super::*;
use crate::persistence::test_support::db;

/// Stand-in for a job's program: the test binary re-run as this test. Its arguments come
/// through `DEVLUMA_FAKE_WORKER_ARGS` (the test harness would reject them).
#[test]
#[ignore = "fake worker process for scheduler tests"]
fn fake_worker() {
    let Ok(args) = std::env::var("DEVLUMA_FAKE_WORKER_ARGS") else {
        return;
    };
    crate::platform::windows::job::tests::exit_with_parent();
    match args.split('\u{1f}').next().unwrap_or_default() {
        "ok" => {
            println!("synced 3 records");
            println!("password=hunter2-secret");
            println!("cwd={}", std::env::current_dir().unwrap().display());
        }
        "fail" => {
            eprintln!("something broke");
            std::process::exit(3);
        }
        "sleep" => std::thread::sleep(Duration::from_secs(60)),
        _ => {}
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<String>>);

impl SchedulerSink for RecordingSink {
    fn job_changed(&self, job_id: &str) {
        self.0.lock().unwrap().push(job_id.to_owned());
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    sched: Arc<Scheduler>,
    project_id: String,
    project_dir: PathBuf,
    worker: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.sched.stop_all();
    }
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths::under(&dir.path().join("Devluma"));
    paths.create_all().unwrap();
    let db = Arc::new(db());
    let project_dir = paths.www.join("shop");
    std::fs::create_dir(&project_dir).unwrap();
    let project = crate::services::projects::create(&db, &paths, "shop", "shop").unwrap();
    let worker = dir.path().join("worker.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &worker).unwrap();
    let launcher = Launcher {
        prefix: vec![
            "--exact".into(),
            "services::scheduler::tests::fake_worker".into(),
            "--ignored".into(),
            "--nocapture".into(),
            "--quiet".into(),
        ],
        env: vec![("DEVLUMA_TEST_PARENT".into(), std::process::id().to_string())],
        args_via_env: true,
    };
    let sched = Scheduler::with(
        db,
        Arc::new(crate::services::catalog::load()),
        paths,
        Arc::new(RecordingSink::default()),
        launcher,
        Some(Duration::from_secs(3)),
    );
    Harness {
        _dir: dir,
        sched,
        project_id: project.id,
        project_dir,
        worker,
    }
}

impl Harness {
    fn request(&self, mode: &str, cron: &str) -> JobRequest {
        JobRequest {
            id: None,
            project_id: self.project_id.clone(),
            name: format!("worker {mode}"),
            kind: JobKind::Command,
            program: Some(self.worker.display().to_string()),
            args: vec![mode.to_owned()],
            cron: cron.to_owned(),
            timeout_secs: 600,
            enabled: true,
        }
    }

    fn save(&self, mode: &str, cron: &str) -> JobView {
        self.sched.save(&self.request(mode, cron)).unwrap()
    }

    fn wait_done(&self, job_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.sched.is_running(job_id) {
            assert!(Instant::now() < deadline, "run didn't finish");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn a_run_records_output_in_the_project_folder_and_redacts_secrets() {
    let h = harness();
    let job = h.save("ok", "* * * * *");
    assert_eq!(job.status, JobStatus::Active);
    assert_eq!(job.schedule_text, "Every minute");
    assert!(job.next_run.is_some());

    let run = h.sched.run_now(&job.id).unwrap();
    assert_eq!(run.trigger, "manual");
    h.wait_done(&job.id);

    let view = h.sched.view(&job.id).unwrap();
    assert_eq!(view.status, JobStatus::Active);
    let last = view.last_run.unwrap();
    assert_eq!(last.status, RunStatus::Succeeded);
    assert_eq!(last.exit_code, Some(0));

    let log = h.sched.run_log(&job.id, &run.id).unwrap();
    assert!(log.contains("synced 3 records"), "{log}");
    assert!(
        !log.contains("hunter2-secret"),
        "secrets are redacted: {log}"
    );
    let cwd = h.project_dir.canonicalize().unwrap();
    assert!(
        log.to_lowercase().contains(
            &cwd.display()
                .to_string()
                .trim_start_matches(r"\\?\")
                .to_lowercase()
        ),
        "runs in the project folder: {log}"
    );
}

#[test]
fn a_non_zero_exit_marks_the_job_failed() {
    let h = harness();
    let job = h.save("fail", "* * * * *");
    h.sched.run_now(&job.id).unwrap();
    h.wait_done(&job.id);
    let view = h.sched.view(&job.id).unwrap();
    assert_eq!(view.status, JobStatus::Failed);
    let last = view.last_run.unwrap();
    assert_eq!(last.status, RunStatus::Failed);
    assert_eq!(last.exit_code, Some(3));
    assert!(
        h.sched
            .run_log(&job.id, &last.id)
            .unwrap()
            .contains("something broke")
    );
}

#[test]
fn the_time_limit_ends_the_whole_run() {
    let h = harness();
    let job = h.save("sleep", "* * * * *");
    h.sched.run_now(&job.id).unwrap();
    assert_eq!(h.sched.view(&job.id).unwrap().status, JobStatus::Executing);
    h.wait_done(&job.id);
    let last = h.sched.view(&job.id).unwrap().last_run.unwrap();
    assert_eq!(last.status, RunStatus::TimedOut);
    assert!(last.message.unwrap().contains("time limit"));
}

#[test]
fn ticks_start_due_jobs_once_skip_overlaps_and_ignore_muted_jobs() {
    let h = harness();
    let busy = h.save("sleep", "* * * * *");
    let muted = h
        .sched
        .save(&JobRequest {
            enabled: false,
            ..h.request("ok", "* * * * *")
        })
        .unwrap();
    assert_eq!(muted.status, JobStatus::Muted);
    assert!(muted.next_run.is_none());
    let later = h.save("ok", "0 3 * * *");

    let at = |m: u32| Local.with_ymd_and_hms(2026, 10, 16, 9, m, 0).unwrap();
    h.sched.tick(at(1));
    assert!(h.sched.is_running(&busy.id), "due job started");
    assert!(
        !h.sched.is_running(&later.id),
        "job due at 03:00 not started"
    );
    assert!(
        h.sched.runs(&muted.id).unwrap().is_empty(),
        "muted job never runs"
    );

    h.sched.tick(at(1)); // the same minute again: nothing new
    assert_eq!(h.sched.runs(&busy.id).unwrap().len(), 1);

    h.sched.tick(at(2)); // still running: skipped, not doubled
    let runs = h.sched.runs(&busy.id).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].status, RunStatus::Skipped);
    assert!(
        h.sched.run_now(&busy.id).is_err(),
        "no overlapping manual run"
    );

    h.sched.stop(&busy.id).unwrap();
    h.wait_done(&busy.id);
    let last = h.sched.view(&busy.id).unwrap().last_run.unwrap();
    assert_eq!(last.status, RunStatus::Stopped);
}

#[test]
fn jobs_are_validated_before_they_are_saved() {
    let h = harness();
    let err = |req: JobRequest| h.sched.save(&req).unwrap_err().message;

    assert!(
        err(JobRequest {
            cron: "61 * * * *".into(),
            ..h.request("ok", "")
        })
        .contains("minute")
    );
    assert!(
        err(JobRequest {
            name: " ".into(),
            ..h.request("ok", "* * * * *")
        })
        .contains("name")
    );
    assert!(
        err(JobRequest {
            timeout_secs: 5,
            ..h.request("ok", "* * * * *")
        })
        .contains("time limit")
    );
    // Only .exe programs: a batch file would need cmd.exe.
    let bat = h.project_dir.join("sync.bat");
    std::fs::write(&bat, "@echo off").unwrap();
    assert!(
        err(JobRequest {
            program: Some(bat.display().to_string()),
            ..h.request("ok", "* * * * *")
        })
        .contains(".exe")
    );
    assert!(
        err(JobRequest {
            program: Some("definitely-not-installed".into()),
            ..h.request("ok", "* * * * *")
        })
        .contains("Couldn't find definitely-not-installed.exe")
    );

    // PHP scripts must be inside the project; artisan jobs need artisan and PHP.
    let php = |args: Vec<String>, kind: JobKind| JobRequest {
        kind,
        program: None,
        args,
        ..h.request("ok", "* * * * *")
    };
    assert!(
        err(php(vec!["..\\outside.php".into()], JobKind::PhpScript)).contains("inside the project")
    );
    assert!(err(php(vec![], JobKind::ArtisanSchedule)).contains("artisan"));
    std::fs::write(h.project_dir.join("artisan"), "<?php").unwrap();
    assert!(err(php(vec![], JobKind::ArtisanSchedule)).contains("PHP"));
}

#[test]
fn preview_shows_exactly_what_will_run_and_when() {
    let h = harness();
    let p = h.sched.preview(&h.request("ok", "*/15  * * * *")).unwrap();
    assert_eq!(p.cron, "*/15 * * * *");
    assert_eq!(p.schedule_text, "Every 15 minutes");
    assert_eq!(p.next_runs.len(), 3);
    assert_eq!(p.command.executable, h.worker.display().to_string());
    assert_eq!(p.command.args, vec!["ok"]);
    assert_eq!(p.command.working_dir, h.project_dir.display().to_string());
}

#[test]
fn deleting_a_job_removes_its_history_and_logs() {
    let h = harness();
    let job = h.save("ok", "* * * * *");
    h.sched.run_now(&job.id).unwrap();
    h.wait_done(&job.id);
    let logs = h.sched.logs_dir(&job.id);
    assert!(logs.is_dir());
    h.sched.delete(&job.id).unwrap();
    assert!(!logs.exists());
    assert!(h.sched.list().unwrap().is_empty());
}
