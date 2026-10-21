-- 0005 — Scheduled jobs (ADR 0017): recurring project tasks Devluma runs while it is open.
-- A job belongs to a project and goes with it. Run output lives in
-- <root>/logs/scheduler/<job id>/<run id>.log (derived, never stored as a path).
CREATE TABLE scheduled_jobs (
  id            TEXT PRIMARY KEY,
  project_id    TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
  name          TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('artisan_schedule', 'php_script', 'command')),
  -- 'command' only: an executable name (found on the project's PATH) or an absolute .exe path.
  program       TEXT NULL,
  -- JSON array of arguments; for 'php_script' the first one is the script, relative to the project.
  args_json     TEXT NOT NULL DEFAULT '[]',
  cron          TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  timeout_secs  INTEGER NOT NULL DEFAULT 600 CHECK (timeout_secs BETWEEN 10 AND 86400),
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
);
CREATE INDEX idx_scheduled_jobs_project ON scheduled_jobs (project_id);

CREATE TABLE scheduled_job_runs (
  id           TEXT PRIMARY KEY,
  job_id       TEXT NOT NULL REFERENCES scheduled_jobs (id) ON DELETE CASCADE,
  trigger      TEXT NOT NULL CHECK (trigger IN ('schedule', 'manual')),
  status       TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'timed_out', 'skipped', 'stopped')),
  exit_code    INTEGER NULL,
  message      TEXT NULL,
  started_at   TEXT NOT NULL,
  finished_at  TEXT NULL
);
CREATE INDEX idx_scheduled_job_runs_job ON scheduled_job_runs (job_id, started_at);
