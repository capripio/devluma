import { TriangleAlert } from "lucide-react";
import { CopyButton } from "../components/CopyButton";
import type { StartupStatus } from "../types/dto";

/**
 * Shown instead of the app when the metadata database couldn't be opened or migrated
 * (app_flow.md §3). Deliberately offers no reset/delete action: the database is never
 * overwritten automatically.
 */
export function RecoveryPage({ status }: { status: StartupStatus }) {
  return (
    <main className="mx-auto flex max-w-2xl flex-col gap-5 p-6">
      <header className="flex items-start gap-3">
        <TriangleAlert aria-hidden className="mt-1 size-6 shrink-0 text-warning" />
        <div>
          <h1 className="text-xl font-semibold">Devluma needs attention before it can start</h1>
          <p className="mt-1 text-sm text-fg">
            {status.error?.message ?? "Devluma's metadata database is unavailable."}
          </p>
        </div>
      </header>

      <section
        aria-labelledby="recovery-safe-heading"
        className="rounded-md border border-border bg-surface-raised p-4 text-sm"
      >
        <h2 id="recovery-safe-heading" className="font-semibold">
          Nothing was deleted
        </h2>
        <p className="mt-1 text-fg-muted">
          Devluma did not modify, reset or delete the database. Your projects, installed runtimes and database
          data are separate files and are unaffected.
        </p>
      </section>

      <section aria-labelledby="recovery-paths-heading" className="text-sm">
        <h2 id="recovery-paths-heading" className="font-semibold">
          Locations
        </h2>
        <dl className="mt-2 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2">
          <dt className="text-fg-muted">Database</dt>
          <dd className="flex min-w-0 items-center gap-1 font-mono break-all">
            {status.databasePath}
            <CopyButton value={status.databasePath} label="database path" />
          </dd>
          <dt className="text-fg-muted">Backups</dt>
          <dd className="flex min-w-0 items-center gap-1 font-mono break-all">
            {status.backupsDir}
            <CopyButton value={status.backupsDir} label="backups folder path" />
          </dd>
        </dl>
      </section>

      <section aria-labelledby="recovery-next-heading" className="text-sm">
        <h2 id="recovery-next-heading" className="font-semibold">
          What you can do
        </h2>
        <ul className="mt-2 list-inside list-disc space-y-1 text-fg-muted">
          <li>
            If you recently switched Devluma versions, reopen it with the version that created this database.
          </li>
          <li>
            Otherwise, close Devluma and replace the database with a copy from the backups folder. Backups are
            named after the schema version they were taken from.
          </li>
          {status.error && (
            <li>
              Error code: <span className="font-mono">{status.error.code}</span>
            </li>
          )}
        </ul>
      </section>
    </main>
  );
}
