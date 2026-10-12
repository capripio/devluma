-- 0002 — Name of the job object owning a service's current run (ADR 0006).
-- A terminated job can't accept new processes, so every start creates a fresh, uniquely
-- named job; the name is recorded so a restarted Devluma can re-open it for adoption.
ALTER TABLE service_instances ADD COLUMN job_name TEXT NULL;
