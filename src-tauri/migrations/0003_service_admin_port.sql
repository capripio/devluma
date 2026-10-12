-- 0003 — Second port for services that expose one (e.g. Mailpit's web UI next to SMTP).
-- `port` stays the port applications connect to; `admin_port` is for the tool's own UI.
ALTER TABLE service_instances ADD COLUMN admin_port INTEGER NULL CHECK (admin_port BETWEEN 1 AND 65535);
