-- 0006 — Simple settings for configs Devluma generates (ADR 0018): nginx, Apache, MongoDB
-- and Mailpit. Rendered into the generated config (or launch options) before every start.
-- Settings that live in a tool's own file (php.ini, my.ini, postgresql.conf) are not stored
-- here; they are edited in that file.
CREATE TABLE service_settings (
  service_id  TEXT NOT NULL REFERENCES service_instances (id) ON DELETE CASCADE,
  key         TEXT NOT NULL,
  value       TEXT NOT NULL,
  updated_at  TEXT NOT NULL,
  PRIMARY KEY (service_id, key)
);
