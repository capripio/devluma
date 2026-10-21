//! Scheduled jobs and their run history (`scheduled_jobs`, `scheduled_job_runs`; ADR 0017).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{DbError, DbResult, new_id, now};

/// How many runs are kept per job; older rows (and their logs) are pruned.
pub const KEEP_RUNS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ArtisanSchedule,
    PhpScript,
    Command,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArtisanSchedule => "artisan_schedule",
            Self::PhpScript => "php_script",
            Self::Command => "command",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "artisan_schedule" => Self::ArtisanSchedule,
            "php_script" => Self::PhpScript,
            _ => Self::Command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub kind: JobKind,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cron: String,
    pub enabled: bool,
    pub timeout_secs: u32,
    pub created_at: String,
    pub updated_at: String,
}

pub struct JobFields<'a> {
    pub project_id: &'a str,
    pub name: &'a str,
    pub kind: JobKind,
    pub program: Option<&'a str>,
    pub args: &'a [String],
    pub cron: &'a str,
    pub enabled: bool,
    pub timeout_secs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Skipped,
    Stopped,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Skipped => "skipped",
            Self::Stopped => "stopped",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "timed_out" => Self::TimedOut,
            "skipped" => Self::Skipped,
            _ => Self::Stopped,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub job_id: String,
    /// `schedule` or `manual` (Run now).
    pub trigger: String,
    pub status: RunStatus,
    pub exit_code: Option<i64>,
    pub message: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

const JOB_COLUMNS: &str = "id, project_id, name, kind, program, args_json, cron, enabled, timeout_secs, created_at, updated_at";
const RUN_COLUMNS: &str =
    "id, job_id, trigger, status, exit_code, message, started_at, finished_at";

fn job_from_row(r: &Row<'_>) -> rusqlite::Result<Job> {
    let args: String = r.get(5)?;
    Ok(Job {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        kind: JobKind::parse(&r.get::<_, String>(3)?),
        program: r.get(4)?,
        args: serde_json::from_str(&args).unwrap_or_default(),
        cron: r.get(6)?,
        enabled: r.get::<_, i64>(7)? != 0,
        timeout_secs: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        job_id: r.get(1)?,
        trigger: r.get(2)?,
        status: RunStatus::parse(&r.get::<_, String>(3)?),
        exit_code: r.get(4)?,
        message: r.get(5)?,
        started_at: r.get(6)?,
        finished_at: r.get(7)?,
    })
}

fn args_json(args: &[String]) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "[]".into())
}

pub fn insert(conn: &Connection, f: &JobFields<'_>) -> DbResult<Job> {
    let id = new_id();
    let ts = now();
    conn.execute(
        "INSERT INTO scheduled_jobs (id, project_id, name, kind, program, args_json, cron, enabled,
                                     timeout_secs, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            id,
            f.project_id,
            f.name,
            f.kind.as_str(),
            f.program,
            args_json(f.args),
            f.cron,
            f.enabled,
            f.timeout_secs,
            ts
        ],
    )?;
    get(conn, &id)
}

pub fn update(conn: &Connection, id: &str, f: &JobFields<'_>) -> DbResult<Job> {
    let n = conn.execute(
        "UPDATE scheduled_jobs SET project_id = ?2, name = ?3, kind = ?4, program = ?5, args_json = ?6,
                                   cron = ?7, enabled = ?8, timeout_secs = ?9, updated_at = ?10
         WHERE id = ?1",
        params![
            id,
            f.project_id,
            f.name,
            f.kind.as_str(),
            f.program,
            args_json(f.args),
            f.cron,
            f.enabled,
            f.timeout_secs,
            now()
        ],
    )?;
    if n == 0 {
        return Err(DbError::NotFound {
            entity: "scheduled job",
        });
    }
    get(conn, id)
}

pub fn set_enabled(conn: &Connection, id: &str, enabled: bool) -> DbResult<Job> {
    let n = conn.execute(
        "UPDATE scheduled_jobs SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, enabled, now()],
    )?;
    if n == 0 {
        return Err(DbError::NotFound {
            entity: "scheduled job",
        });
    }
    get(conn, id)
}

pub fn get(conn: &Connection, id: &str) -> DbResult<Job> {
    conn.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM scheduled_jobs WHERE id = ?1"),
        [id],
        job_from_row,
    )
    .optional()?
    .ok_or(DbError::NotFound {
        entity: "scheduled job",
    })
}

pub fn list(conn: &Connection) -> DbResult<Vec<Job>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {JOB_COLUMNS} FROM scheduled_jobs ORDER BY name COLLATE NOCASE"
    ))?;
    Ok(stmt
        .query_map([], job_from_row)?
        .collect::<Result<_, _>>()?)
}

pub fn delete(conn: &Connection, id: &str) -> DbResult<()> {
    conn.execute("DELETE FROM scheduled_jobs WHERE id = ?1", [id])?;
    Ok(())
}

pub fn start_run(conn: &Connection, job_id: &str, trigger: &str) -> DbResult<Run> {
    insert_run(conn, job_id, trigger, RunStatus::Running, None)
}

/// A tick that couldn't start (the previous run was still going, or the job can't run).
pub fn record_not_started(
    conn: &Connection,
    job_id: &str,
    trigger: &str,
    status: RunStatus,
    message: &str,
) -> DbResult<Run> {
    let run = insert_run(conn, job_id, trigger, status, Some(message))?;
    conn.execute(
        "UPDATE scheduled_job_runs SET finished_at = started_at WHERE id = ?1",
        [&run.id],
    )?;
    get_run(conn, &run.id)
}

fn insert_run(
    conn: &Connection,
    job_id: &str,
    trigger: &str,
    status: RunStatus,
    message: Option<&str>,
) -> DbResult<Run> {
    let id = new_id();
    conn.execute(
        "INSERT INTO scheduled_job_runs (id, job_id, trigger, status, message, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, job_id, trigger, status.as_str(), message, now()],
    )?;
    get_run(conn, &id)
}

pub fn finish_run(
    conn: &Connection,
    run_id: &str,
    status: RunStatus,
    exit_code: Option<i64>,
    message: Option<&str>,
) -> DbResult<Run> {
    conn.execute(
        "UPDATE scheduled_job_runs SET status = ?2, exit_code = ?3, message = ?4, finished_at = ?5
         WHERE id = ?1",
        params![run_id, status.as_str(), exit_code, message, now()],
    )?;
    get_run(conn, run_id)
}

pub fn get_run(conn: &Connection, id: &str) -> DbResult<Run> {
    conn.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE id = ?1"),
        [id],
        run_from_row,
    )
    .optional()?
    .ok_or(DbError::NotFound { entity: "job run" })
}

/// Newest first.
pub fn runs(conn: &Connection, job_id: &str, limit: usize) -> DbResult<Vec<Run>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM scheduled_job_runs WHERE job_id = ?1
         ORDER BY started_at DESC, id DESC LIMIT ?2"
    ))?;
    Ok(stmt
        .query_map(params![job_id, limit as i64], run_from_row)?
        .collect::<Result<_, _>>()?)
}

/// The latest run that ran to an end (skipped ticks don't change a job's health).
pub fn last_finished(conn: &Connection, job_id: &str) -> DbResult<Option<Run>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {RUN_COLUMNS} FROM scheduled_job_runs
                 WHERE job_id = ?1 AND status NOT IN ('running', 'skipped')
                 ORDER BY started_at DESC, id DESC LIMIT 1"
            ),
            [job_id],
            run_from_row,
        )
        .optional()?)
}

/// Deletes runs beyond the newest `KEEP_RUNS` and returns their ids (to remove their logs).
pub fn prune_runs(conn: &Connection, job_id: &str) -> DbResult<Vec<String>> {
    let old: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM scheduled_job_runs WHERE job_id = ?1
             ORDER BY started_at DESC, id DESC LIMIT -1 OFFSET ?2",
        )?;
        stmt.query_map(params![job_id, KEEP_RUNS as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for id in &old {
        conn.execute("DELETE FROM scheduled_job_runs WHERE id = ?1", [id])?;
    }
    Ok(old)
}

/// Runs still marked running from a previous session: Devluma closed while they ran.
pub fn close_interrupted(conn: &Connection) -> DbResult<usize> {
    Ok(conn.execute(
        "UPDATE scheduled_job_runs
         SET status = 'stopped', message = 'Devluma closed while this ran.', finished_at = ?1
         WHERE status = 'running'",
        [now()],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::test_support::db;

    fn project(db: &crate::persistence::Database) -> String {
        db.write(|c| {
            c.execute(
                "INSERT INTO projects (id, name, root_path, is_favorite, created_at, updated_at)
                 VALUES ('p1', 'shop', 'C:\\Projects\\shop', 0, 't', 't')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        "p1".into()
    }

    fn fields<'a>(project_id: &'a str, args: &'a [String]) -> JobFields<'a> {
        JobFields {
            project_id,
            name: "Laravel scheduler",
            kind: JobKind::ArtisanSchedule,
            program: None,
            args,
            cron: "* * * * *",
            enabled: true,
            timeout_secs: 600,
        }
    }

    #[test]
    fn jobs_and_runs_round_trip_and_prune() {
        let db = db();
        let p = project(&db);
        let args = vec!["schedule:run".to_owned()];
        let job = db.write(|c| insert(c, &fields(&p, &args))).unwrap();
        assert_eq!(job.args, args);
        assert_eq!(job.kind, JobKind::ArtisanSchedule);

        let muted = db.write(|c| set_enabled(c, &job.id, false)).unwrap();
        assert!(!muted.enabled);

        for i in 0..(KEEP_RUNS + 3) {
            db.write(|c| {
                let r = start_run(c, &job.id, "schedule")?;
                let status = if i % 2 == 0 {
                    RunStatus::Succeeded
                } else {
                    RunStatus::Failed
                };
                finish_run(c, &r.id, status, Some(i as i64), None)?;
                Ok(())
            })
            .unwrap();
        }
        let pruned = db.write(|c| prune_runs(c, &job.id)).unwrap();
        assert_eq!(pruned.len(), 3);
        assert_eq!(db.read(|c| runs(c, &job.id, 100)).unwrap().len(), KEEP_RUNS);

        db.write(|c| {
            record_not_started(c, &job.id, "schedule", RunStatus::Skipped, "still running")
        })
        .unwrap();
        let last = db.read(|c| last_finished(c, &job.id)).unwrap().unwrap();
        assert_ne!(last.status, RunStatus::Skipped, "skipped ticks don't count");
    }

    #[test]
    fn interrupted_runs_are_closed_and_projects_take_their_jobs() {
        let db = db();
        let p = project(&db);
        let job = db.write(|c| insert(c, &fields(&p, &[]))).unwrap();
        let run = db.write(|c| start_run(c, &job.id, "manual")).unwrap();
        assert_eq!(db.write(|c| close_interrupted(c)).unwrap(), 1);
        let run = db.read(|c| get_run(c, &run.id)).unwrap();
        assert_eq!(run.status, RunStatus::Stopped);
        assert!(run.finished_at.is_some());

        db.write(|c| {
            c.execute("DELETE FROM projects WHERE id = ?1", [&p])?;
            Ok(())
        })
        .unwrap();
        assert!(db.read(list).unwrap().is_empty());
    }
}
