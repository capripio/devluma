import { Database } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router";
import { useOnOperationsSettled } from "../../app/operations";
import { Button } from "../../components/Button";
import { CopyButton } from "../../components/CopyButton";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { formatBytes } from "../../lib/format";
import { useAsync } from "../../lib/useAsync";
import type { BackupRecord, DatabaseServiceView } from "../../types/dto";
import { RestoreDialog } from "./RestoreDialog";
import { LoadingState } from "../../components/LoadingState";

const ENGINE_NAMES: Record<DatabaseServiceView["engine"], string> = {
  mariadb: "MariaDB",
  postgresql: "PostgreSQL",
  mongodb: "MongoDB",
};

export function DatabasesPage() {
  const services = useAsync(ipc.listDatabaseServices);
  const backups = useAsync(ipc.listDatabaseBackups);
  const refresh = () => {
    services.refresh();
    backups.refresh();
  };
  useOnOperationsSettled(
    (op) =>
      ["backup_database", "snapshot_database", "restore_database", "start_service", "stop_service"].includes(
        op.operationType,
      ),
    refresh,
  );
  return (
    <>
      <PageHeader title="Databases" description="Connection details, backups and restores." />
      {services.status === "pending" && <LoadingState label="Loading databases…" shape="table" columns={5} />}
      {services.status === "error" && <ErrorPanel error={services.error} onRetry={services.reload} />}
      {services.status === "success" &&
        (services.data.length === 0 ? (
          <EmptyState
            icon={Database}
            title="No database services"
            description="Install MariaDB, PostgreSQL or MongoDB, then add it on the Services page. Existing data is never overwritten."
          />
        ) : (
          services.data.map((d) => <DatabaseCard key={d.serviceId} db={d} onChanged={refresh} />)
        ))}
      {backups.status === "success" && services.status === "success" && (
        <BackupsTable backups={backups.data} services={services.data} onChanged={refresh} />
      )}
    </>
  );
}

function DatabaseCard({ db, onChanged }: { db: DatabaseServiceView; onChanged: () => void }) {
  const running = db.observedState === "running";
  const headingId = `db-${db.serviceId}`;
  const c = db.connection;
  return (
    <section
      aria-labelledby={headingId}
      className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm"
    >
      <h2 id={headingId} className="font-semibold">
        <Link to={`/services/${db.serviceId}`} className="text-accent hover:underline">
          {db.name}
        </Link>{" "}
        <span className="font-normal text-fg-muted">
          {ENGINE_NAMES[db.engine]} {db.version} · {db.observedState}
        </span>
      </h2>
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
        {c && (
          <>
            <dt className="text-fg-muted">Address</dt>
            <dd className="font-mono">
              {c.host}:{c.port}
              {c.user && <span className="font-sans text-fg-muted"> · user {c.user}</span>}
            </dd>
            <dt className="text-fg-muted">URL</dt>
            <dd className="flex items-center gap-1 font-mono break-all">
              {c.url}
              <CopyButton value={c.url} label={`connection URL for ${db.name}`} />
            </dd>
            {c.cli && (
              <>
                <dt className="text-fg-muted">Command line</dt>
                <dd className="flex items-center gap-1 font-mono text-xs break-all">
                  {c.cli}
                  <CopyButton value={c.cli} label={`client command for ${db.name}`} />
                </dd>
              </>
            )}
          </>
        )}
        <dt className="text-fg-muted">Data folder</dt>
        <dd className="font-mono break-all">
          {db.dataPath ?? "—"}
          {db.dataPath && !db.dataInitialized && (
            <span className="ml-2 font-sans text-warning">
              {db.engine === "mongodb" ? "(created on first start)" : "(not initialized)"}
            </span>
          )}
        </dd>
      </dl>
      {c && <p className="text-fg-muted">{c.credentialNote}</p>}
      <BackupActions db={db} running={running} onChanged={onChanged} />
    </section>
  );
}

function BackupActions({
  db,
  running,
  onChanged,
}: {
  db: DatabaseServiceView;
  running: boolean;
  onChanged: () => void;
}) {
  const names = useAsync(() =>
    db.logicalBackups && running ? ipc.listDatabases(db.serviceId) : Promise.resolve([]),
  );
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState<"backup" | "snapshot" | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const live = ["starting", "running", "degraded", "stopping"].includes(db.observedState);
  const options = names.status === "success" ? names.data : [];
  const database = selected || options[0] || "";

  const run = (kind: "backup" | "snapshot", action: () => Promise<BackupRecord>) => {
    setBusy(kind);
    setError(null);
    setDone(null);
    action().then(
      (b) => {
        setBusy(null);
        setDone(`Saved to ${b.path}`);
        onChanged();
      },
      (e: unknown) => {
        setBusy(null);
        setError(toAppError(e));
      },
    );
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-end gap-3">
        {db.logicalBackups && (
          <>
            <label className="flex flex-col gap-1">
              <span className="text-fg-muted">Database</span>
              <select
                value={database}
                onChange={(e) => setSelected(e.target.value)}
                disabled={!running || options.length === 0}
                className="rounded-md border border-border bg-surface px-2 py-1.5"
              >
                {options.length === 0 && (
                  <option value="">{running ? "No databases" : "Start the server"}</option>
                )}
                {options.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </label>
            <Button
              pending={busy === "backup"}
              disabled={!running || !database || busy !== null}
              onClick={() => run("backup", () => ipc.backupDatabase(db.serviceId, database))}
              aria-label={`Back up ${database || "a database"} on ${db.name}`}
            >
              Back up database
            </Button>
          </>
        )}
        <Button
          pending={busy === "snapshot"}
          disabled={live || busy !== null}
          onClick={() => run("snapshot", () => ipc.snapshotDatabase(db.serviceId))}
          aria-label={`Snapshot the data folder of ${db.name}`}
        >
          Snapshot data folder
        </Button>
      </div>
      <p className="text-xs text-fg-muted">
        {db.logicalBackups
          ? "Database backups use the server's own dump tool while it runs. Snapshots copy the whole data folder while the service is stopped."
          : "MongoDB backups are data folder snapshots, taken while the service is stopped."}
      </p>
      {done && (
        <p role="status" className="text-xs text-fg-muted">
          {done}
        </p>
      )}
      {error && <ErrorPanel error={error} />}
    </div>
  );
}

function BackupsTable({
  backups,
  services,
  onChanged,
}: {
  backups: BackupRecord[];
  services: DatabaseServiceView[];
  onChanged: () => void;
}) {
  const [restoring, setRestoring] = useState<BackupRecord | null>(null);
  return (
    <section aria-labelledby="backups-heading" className="flex flex-col gap-2">
      <h2 id="backups-heading" className="text-sm font-semibold">
        Backups
      </h2>
      {backups.length === 0 ? (
        <p className="text-sm text-fg-muted">
          No backups yet. Backups are kept when services or packages are removed.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full text-left text-sm">
            <thead className="bg-surface-raised text-fg-muted">
              <tr>
                {["Created", "From", "Contents", "Size", "File", "Action"].map((h) => (
                  <th key={h} scope="col" className="px-3 py-2 font-medium">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {backups.map((b) => (
                <tr key={b.id} className="align-top">
                  <td className="px-3 py-2 whitespace-nowrap">{new Date(b.createdAt).toLocaleString()}</td>
                  <td className="px-3 py-2">{b.serviceName}</td>
                  <td className="px-3 py-2">
                    {b.kind === "logical" ? `Database ${b.databaseName}` : "Whole data folder"}
                    {b.note && <div className="text-xs text-fg-muted">{b.note}</div>}
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">{formatBytes(b.sizeBytes)}</td>
                  <td className="px-3 py-2 font-mono text-xs break-all">{b.path}</td>
                  <td className="px-3 py-2">
                    <Button onClick={() => setRestoring(b)} aria-label={`Restore backup from ${b.createdAt}`}>
                      Restore…
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {restoring && (
        <RestoreDialog
          backup={restoring}
          services={services.filter((s) => s.engine === restoring.engine)}
          onClose={() => setRestoring(null)}
          onRestored={onChanged}
        />
      )}
    </section>
  );
}
