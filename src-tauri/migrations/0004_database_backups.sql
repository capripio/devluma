-- 0004 — Database backups made by Devluma (app_flow.md §9). Files live under
-- <root>/backups/databases/; this table records what each one is. Rows outlive the service
-- they came from (no foreign key), because backups are kept when a service is removed.
CREATE TABLE database_backups (
  id                   TEXT PRIMARY KEY,
  service_instance_id  TEXT NULL,
  service_name         TEXT NOT NULL,
  engine               TEXT NOT NULL CHECK (engine IN ('mariadb', 'postgresql', 'mongodb')),
  kind                 TEXT NOT NULL CHECK (kind IN ('logical', 'snapshot')),
  database_name        TEXT NULL,
  path                 TEXT NOT NULL,
  size_bytes           INTEGER NOT NULL,
  sha256               TEXT NULL,
  note                 TEXT NULL,
  operation_id         TEXT NULL,
  created_at           TEXT NOT NULL
);
CREATE INDEX idx_database_backups_service ON database_backups (service_instance_id, created_at);
