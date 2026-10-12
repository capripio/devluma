-- 0001 — Initial schema (schema.md §3, §6).
-- Migrations are forward-only and checksummed: never edit an applied migration; add a new one.
-- IDs are ULID text; timestamps are UTC RFC 3339 text (lexicographically ordered).
-- `schema_migrations` itself is created by the migration runner, not here.

CREATE TABLE projects (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL,
  root_path       TEXT NOT NULL,
  project_type    TEXT NULL,
  local_domain    TEXT NULL,
  notes           TEXT NULL,
  is_favorite     INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0, 1)),
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL,
  last_opened_at  TEXT NULL
);
CREATE INDEX idx_projects_updated_at ON projects (updated_at);

CREATE TABLE packages (
  id                  TEXT PRIMARY KEY,
  display_name        TEXT NOT NULL,
  category            TEXT NOT NULL,
  description         TEXT NULL,
  publisher           TEXT NULL,
  homepage_url        TEXT NULL,
  license_id          TEXT NULL,
  license_url         TEXT NULL,
  catalog_source      TEXT NOT NULL,
  catalog_version     TEXT NOT NULL,
  support_policy_url  TEXT NULL,
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);

-- catalog_version stays on `packages`, so versions are unique per (package, version, architecture).
CREATE TABLE package_versions (
  id                       TEXT PRIMARY KEY,
  package_id               TEXT NOT NULL REFERENCES packages (id),
  version                  TEXT NOT NULL,
  architecture             TEXT NOT NULL CHECK (architecture IN ('x64', 'arm64', 'any')),
  os_constraints_json      TEXT NOT NULL,
  source_url               TEXT NOT NULL,
  publisher_signature_url  TEXT NULL,
  sha256                   TEXT NULL,
  integrity_policy         TEXT NOT NULL,
  archive_type             TEXT NOT NULL,
  expected_size_bytes      INTEGER NULL CHECK (expected_size_bytes IS NULL OR expected_size_bytes >= 0),
  release_date             TEXT NULL,
  end_of_life_date         TEXT NULL,
  manifest_json            TEXT NOT NULL,
  created_at               TEXT NOT NULL,
  UNIQUE (package_id, version, architecture)
);

CREATE TABLE package_installations (
  id                  TEXT PRIMARY KEY,
  package_version_id  TEXT NOT NULL REFERENCES package_versions (id),
  install_path        TEXT NOT NULL,
  status              TEXT NOT NULL CHECK (status IN ('installing', 'installed', 'broken', 'uninstalling', 'removed')),
  verified_at         TEXT NULL,
  integrity_result    TEXT NOT NULL,
  installed_at        TEXT NULL,
  last_used_at        TEXT NULL,
  is_global_default   INTEGER NOT NULL DEFAULT 0 CHECK (is_global_default IN (0, 1)),
  metadata_json       TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_package_installations_status ON package_installations (status, package_version_id);

CREATE TABLE project_runtimes (
  id                       TEXT PRIMARY KEY,
  project_id               TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
  package_id               TEXT NOT NULL REFERENCES packages (id),
  package_installation_id  TEXT NOT NULL REFERENCES package_installations (id),
  selection_scope          TEXT NOT NULL CHECK (selection_scope IN ('project')),
  created_at               TEXT NOT NULL,
  UNIQUE (project_id, package_id)
);

CREATE TABLE service_instances (
  id                       TEXT PRIMARY KEY,
  package_installation_id  TEXT NOT NULL REFERENCES package_installations (id),
  instance_name            TEXT NOT NULL,
  service_type             TEXT NOT NULL,
  lifecycle_adapter        TEXT NOT NULL,
  desired_state            TEXT NOT NULL,
  observed_state           TEXT NOT NULL CHECK (observed_state IN (
                             'not_installed', 'stopped', 'starting', 'running',
                             'degraded', 'stopping', 'failed', 'unknown')),
  port                     INTEGER NULL CHECK (port BETWEEN 1 AND 65535),
  bind_address             TEXT NULL,
  config_path              TEXT NULL,
  data_path                TEXT NULL,
  pid                      INTEGER NULL,
  process_started_at       TEXT NULL,
  health_status            TEXT NOT NULL DEFAULT 'unknown',
  last_exit_code           INTEGER NULL,
  last_error_code          TEXT NULL,
  updated_at               TEXT NOT NULL,
  UNIQUE (package_installation_id, instance_name)
);
CREATE INDEX idx_service_instances_state ON service_instances (observed_state, service_type);

CREATE TABLE service_dependencies (
  service_instance_id    TEXT NOT NULL REFERENCES service_instances (id) ON DELETE CASCADE,
  depends_on_service_id  TEXT NOT NULL REFERENCES service_instances (id),
  dependency_kind        TEXT NOT NULL CHECK (dependency_kind IN ('required', 'optional')),
  PRIMARY KEY (service_instance_id, depends_on_service_id),
  CHECK (service_instance_id <> depends_on_service_id)
);

CREATE TABLE project_service_links (
  project_id           TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
  service_instance_id  TEXT NOT NULL REFERENCES service_instances (id),
  purpose              TEXT NOT NULL,
  created_at           TEXT NOT NULL,
  PRIMARY KEY (project_id, service_instance_id, purpose)
);

CREATE TABLE sites (
  id                     TEXT PRIMARY KEY,
  project_id             TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
  domain                 TEXT NOT NULL,
  document_root          TEXT NOT NULL,
  web_server_service_id  TEXT NOT NULL REFERENCES service_instances (id),
  php_installation_id    TEXT NULL REFERENCES package_installations (id),
  http_port              INTEGER NOT NULL DEFAULT 80 CHECK (http_port BETWEEN 1 AND 65535),
  https_port             INTEGER NULL CHECK (https_port BETWEEN 1 AND 65535),
  ssl_enabled            INTEGER NOT NULL DEFAULT 0 CHECK (ssl_enabled IN (0, 1)),
  config_path            TEXT NOT NULL,
  config_hash            TEXT NULL,
  status                 TEXT NOT NULL DEFAULT 'draft',
  hosts_managed          INTEGER NOT NULL DEFAULT 0 CHECK (hosts_managed IN (0, 1)),
  created_at             TEXT NOT NULL,
  updated_at             TEXT NOT NULL
);
CREATE INDEX idx_sites_domain_status ON sites (domain, status);

CREATE TABLE certificates (
  id                  TEXT PRIMARY KEY,
  common_name         TEXT NOT NULL,
  sans_json           TEXT NOT NULL,
  certificate_path    TEXT NOT NULL,
  private_key_path    TEXT NOT NULL,
  issuer_type         TEXT NOT NULL,
  fingerprint_sha256  TEXT NOT NULL,
  not_before          TEXT NOT NULL,
  not_after           TEXT NOT NULL,
  trust_store         TEXT NULL,
  trusted             INTEGER NOT NULL DEFAULT 0 CHECK (trusted IN (0, 1)),
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);

CREATE TABLE certificate_bindings (
  site_id         TEXT NOT NULL REFERENCES sites (id) ON DELETE CASCADE,
  certificate_id  TEXT NOT NULL REFERENCES certificates (id),
  created_at      TEXT NOT NULL,
  PRIMARY KEY (site_id, certificate_id)
);

CREATE TABLE operations (
  id                  TEXT PRIMARY KEY,
  operation_type      TEXT NOT NULL,
  target_type         TEXT NULL,
  target_id           TEXT NULL,
  status              TEXT NOT NULL CHECK (status IN (
                        'queued', 'running', 'succeeded', 'failed', 'cancelled', 'needs_recovery')),
  phase               TEXT NULL,
  progress_current    INTEGER NULL CHECK (progress_current IS NULL OR progress_current >= 0),
  progress_total      INTEGER NULL CHECK (progress_total IS NULL OR progress_total > 0),
  started_at          TEXT NOT NULL,
  finished_at         TEXT NULL,
  error_code          TEXT NULL,
  safe_error_message  TEXT NULL,
  recovery_data_json  TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX idx_operations_status_started ON operations (status, started_at);

CREATE TABLE operation_events (
  id            TEXT PRIMARY KEY,
  operation_id  TEXT NOT NULL REFERENCES operations (id) ON DELETE CASCADE,
  sequence      INTEGER NOT NULL,
  event_type    TEXT NOT NULL,
  level         TEXT NOT NULL CHECK (level IN ('debug', 'info', 'warn', 'error')),
  message       TEXT NOT NULL,
  details_json  TEXT NOT NULL DEFAULT '{}',
  created_at    TEXT NOT NULL,
  UNIQUE (operation_id, sequence)
);

CREATE TABLE hosts_backups (
  id             TEXT PRIMARY KEY,
  original_path  TEXT NOT NULL,
  backup_path    TEXT NOT NULL,
  sha256_before  TEXT NOT NULL,
  sha256_after   TEXT NULL,
  reason         TEXT NOT NULL,
  operation_id   TEXT NULL REFERENCES operations (id),
  created_at     TEXT NOT NULL,
  verified_at    TEXT NULL
);

CREATE TABLE hosts_changes (
  id          TEXT PRIMARY KEY,
  backup_id   TEXT NOT NULL REFERENCES hosts_backups (id),
  domain      TEXT NOT NULL,
  ip_address  TEXT NOT NULL,
  action      TEXT NOT NULL CHECK (action IN ('add', 'update', 'remove')),
  status      TEXT NOT NULL,
  created_at  TEXT NOT NULL
);
CREATE INDEX idx_hosts_changes_domain ON hosts_changes (domain, created_at);

CREATE TABLE settings (
  key         TEXT PRIMARY KEY,
  value_json  TEXT NOT NULL,
  updated_at  TEXT NOT NULL
);

CREATE TABLE secret_references (
  id          TEXT PRIMARY KEY,
  provider    TEXT NOT NULL,
  opaque_key  TEXT NOT NULL,
  label       TEXT NOT NULL,
  created_at  TEXT NOT NULL
);

CREATE TABLE secret_usage (
  secret_reference_id  TEXT NOT NULL REFERENCES secret_references (id) ON DELETE CASCADE,
  target_type          TEXT NOT NULL,
  target_id            TEXT NOT NULL,
  purpose              TEXT NOT NULL,
  PRIMARY KEY (secret_reference_id, target_type, target_id, purpose)
);
