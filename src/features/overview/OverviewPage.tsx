import { ListChecks, Server, TriangleAlert } from "lucide-react";
import { Link } from "react-router";
import { describeOperationType, needsRecovery, useOperations } from "../../app/operations";
import { useStartupStatus } from "../../app/startup";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { OperationStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";

const RECENT_LIMIT = 8;

// No fabricated metrics (design.md §5): sections show real data or say it isn't tracked yet.
export function OverviewPage() {
  return (
    <>
      <PageHeader title="Overview" description="Service health, runtimes, projects and recent activity." />
      <StartupNotice />
      <EmptyState
        icon={Server}
        title="Nothing is managed yet"
        description="Once you install runtimes and services, their status, ports and health will appear here."
        action={
          <Link
            to="/runtimes"
            className="inline-flex rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:bg-accent-hover"
          >
            Go to Runtimes & Packages
          </Link>
        }
      />
      <RecentOperations />
      <AppInfoCard />
    </>
  );
}

/** What startup reconciliation changed, so nothing is silently re-labelled. */
function StartupNotice() {
  const recovery = useStartupStatus()?.recovery;
  if (!recovery) return null;
  const lines = [
    recovery.operationsNeedingRecovery > 0 &&
      `${recovery.operationsNeedingRecovery} operation${recovery.operationsNeedingRecovery === 1 ? " was" : "s were"} interrupted when Devluma last closed. Review before retrying.`,
    recovery.installationsMarkedBroken > 0 &&
      `${recovery.installationsMarkedBroken} installed package${recovery.installationsMarkedBroken === 1 ? " is" : "s are"} incomplete or missing files and ${recovery.installationsMarkedBroken === 1 ? "was" : "were"} marked broken.`,
  ].filter(Boolean);
  if (lines.length === 0) return null;
  return (
    <section
      aria-label="Startup checks"
      className="flex gap-3 rounded-lg border border-warning/40 bg-warning-subtle p-4 text-sm"
    >
      <TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning" />
      <ul className="flex flex-col gap-1 text-fg">
        {lines.map((l) => (
          <li key={String(l)}>{l}</li>
        ))}
      </ul>
    </section>
  );
}

function RecentOperations() {
  const { operations, loadError } = useOperations();
  const recent = operations.slice(0, RECENT_LIMIT);
  return (
    <section aria-labelledby="recent-ops-heading" className="flex flex-col gap-3">
      <h2 id="recent-ops-heading" className="text-sm font-semibold">
        Recent operations
      </h2>
      {loadError ? (
        <ErrorPanel error={loadError} />
      ) : recent.length === 0 ? (
        <p className="flex items-center gap-2 text-sm text-fg-muted">
          <ListChecks aria-hidden className="size-4" />
          No operations yet.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full text-left text-sm">
            <thead className="bg-surface-raised text-fg-muted">
              <tr>
                <th scope="col" className="px-3 py-2 font-medium">
                  Operation
                </th>
                <th scope="col" className="px-3 py-2 font-medium">
                  Status
                </th>
                <th scope="col" className="px-3 py-2 font-medium">
                  Started
                </th>
                <th scope="col" className="px-3 py-2 font-medium">
                  Details
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {recent.map((op) => (
                <tr key={op.operationId}>
                  <td className="px-3 py-2">{describeOperationType(op.operationType)}</td>
                  <td className="px-3 py-2">
                    <OperationStatusBadge status={op.status} />
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">
                    <time dateTime={op.startedAt}>{formatTime(op.startedAt)}</time>
                  </td>
                  <td className="px-3 py-2 text-fg-muted">
                    {needsRecovery(op)
                      ? "Interrupted when Devluma closed"
                      : (op.safeMessage ?? op.phase ?? "")}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

function AppInfoCard() {
  const info = useAsync(ipc.getAppInfo);
  return (
    <section aria-labelledby="app-info-heading" className="rounded-lg border border-border p-4">
      <h2 id="app-info-heading" className="mb-3 text-sm font-semibold">
        This installation
      </h2>
      {info.status === "pending" && (
        <p role="status" className="text-sm text-fg-muted">
          Loading app details…
        </p>
      )}
      {info.status === "error" && <ErrorPanel error={info.error} onRetry={info.reload} />}
      {info.status === "success" && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm">
          <dt className="text-fg-muted">Version</dt>
          <dd className="font-mono">
            {info.data.version}
            {info.data.debugBuild && " (debug build)"}
          </dd>
          <dt className="text-fg-muted">Platform</dt>
          <dd className="font-mono">
            {info.data.os} / {info.data.arch}
          </dd>
        </dl>
      )}
    </section>
  );
}
