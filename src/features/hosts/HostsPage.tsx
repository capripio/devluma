import { TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { useOnOperationsSettled } from "../../app/operations";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { DiffViewer } from "../../components/DiffViewer";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { PermissionExplainer } from "../../components/PermissionExplainer";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { HostsChangeRequest, HostsPlan, HostsView } from "../../types/dto";

export function HostsPage() {
  const hosts = useAsync(ipc.getHosts);
  useOnOperationsSettled(
    (op) => ["apply_hosts", "apply_site", "remove_site"].includes(op.operationType),
    hosts.refresh,
  );
  return (
    <>
      <PageHeader
        title="Hosts File"
        description="Entries in the Windows hosts file. Devluma only edits its own marked section."
      />
      {hosts.status === "pending" && (
        <p role="status" className="text-sm text-fg-muted">
          Reading the hosts file…
        </p>
      )}
      {hosts.status === "error" && <ErrorPanel error={hosts.error} onRetry={hosts.reload} />}
      {hosts.status === "success" && <Hosts view={hosts.data} />}
    </>
  );
}

function Hosts({ view }: { view: HostsView }) {
  const [raw, setRaw] = useState(false);
  const [change, setChange] = useState<{ title: string; request: HostsChangeRequest } | null>(null);
  return (
    <>
      <p className="flex items-center gap-1 font-mono text-xs break-all text-fg-muted">
        {view.path}
        <CopyButton value={view.path} label="hosts file path" />
      </p>
      {view.problem && (
        <section
          aria-label="Hosts file problem"
          className="flex gap-3 rounded-lg border border-warning/40 bg-warning-subtle p-4 text-sm"
        >
          <TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning" />
          <p>{view.problem}</p>
        </section>
      )}
      <div className="flex gap-2">
        <Button onClick={() => setRaw((r) => !r)} aria-pressed={raw}>
          {raw ? "Show table" : "Show raw file"}
        </Button>
      </div>
      {raw ? (
        <pre
          aria-label="Raw hosts file"
          className="max-h-96 overflow-auto rounded-md bg-surface-sunken p-3 font-mono text-xs"
        >
          {view.raw}
        </pre>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full text-left text-sm">
            <thead className="bg-surface-raised text-fg-muted">
              <tr>
                {["Line", "Address", "Names", "Comment", "Managed by", ""].map((h, i) => (
                  <th key={i} scope="col" className="px-3 py-2 font-medium">
                    {h || <span className="sr-only">Actions</span>}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {view.entries.length === 0 && (
                <tr>
                  <td colSpan={6} className="px-3 py-3 text-fg-muted">
                    No active entries (comments only).
                  </td>
                </tr>
              )}
              {view.entries.map((e) => (
                <tr key={e.line}>
                  <td className="px-3 py-2 font-mono text-xs">{e.line}</td>
                  <td className="px-3 py-2 font-mono">{e.ip}</td>
                  <td className="px-3 py-2 font-mono">{e.hostnames.join(" ")}</td>
                  <td className="px-3 py-2 text-fg-muted">{e.comment ?? ""}</td>
                  <td className="px-3 py-2">{e.managed ? "Devluma" : "Other / you"}</td>
                  <td className="px-3 py-2">
                    {e.managed && (
                      <Button
                        onClick={() =>
                          setChange({
                            title: `Remove ${e.hostnames.join(", ")} from the hosts file?`,
                            request: { removeDomains: e.hostnames },
                          })
                        }
                        aria-label={`Remove ${e.hostnames.join(" ")}`}
                      >
                        Remove
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <section aria-labelledby="backups-heading" className="flex flex-col gap-2">
        <h2 id="backups-heading" className="text-sm font-semibold">
          Backups
        </h2>
        {view.backups.length === 0 ? (
          <p className="text-sm text-fg-muted">Devluma hasn&apos;t changed the hosts file yet.</p>
        ) : (
          <ul className="divide-y divide-border rounded-lg border border-border text-sm">
            {view.backups.map((b) => (
              <li key={b.id} className="flex flex-wrap items-center gap-3 px-3 py-2">
                <time dateTime={b.createdAt} className="whitespace-nowrap">
                  {new Date(b.createdAt).toLocaleString()}
                </time>
                <span className="min-w-0 flex-1">
                  {b.reason}
                  <span className="ml-2 text-fg-muted">
                    {b.changes.map((c) => `${c.action} ${c.domain}`).join(", ")}
                  </span>
                </span>
                <Button
                  onClick={() =>
                    setChange({
                      title: "Restore Devluma's entries from this backup?",
                      request: { restoreBackupId: b.id },
                    })
                  }
                  aria-label={`Restore entries from backup taken ${new Date(b.createdAt).toLocaleString()}`}
                >
                  Restore
                </Button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {change && (
        <HostsChangeDialog title={change.title} request={change.request} onClose={() => setChange(null)} />
      )}
    </>
  );
}

/** Previews a hosts change (exact diff), then applies exactly what was previewed. */
function HostsChangeDialog({
  title,
  request,
  onClose,
}: {
  title: string;
  request: HostsChangeRequest;
  onClose: () => void;
}) {
  const [plan, setPlan] = useState<HostsPlan | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const key = JSON.stringify(request);

  useEffect(() => {
    let active = true;
    ipc.previewHostsChange(JSON.parse(key) as HostsChangeRequest).then(
      (p) => active && setPlan(p),
      (e: unknown) => active && setError(toAppError(e)),
    );
    return () => {
      active = false;
    };
  }, [key]);

  return (
    <ConfirmChangeDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={title}
      target={plan?.path ?? "hosts file"}
      confirmDisabled={!plan || plan.unchanged}
      impact={
        <div className="flex flex-col gap-2">
          {!plan && !error && <p role="status">Preparing the preview…</p>}
          {error && <ErrorPanel error={error} />}
          {plan &&
            (plan.unchanged ? <p>Nothing would change.</p> : <DiffViewer diff={plan.diff} label="Changes" />)}
          {plan?.needsElevation && !plan.unchanged && (
            <PermissionExplainer>Only Devluma&apos;s own section of the file changes.</PermissionExplainer>
          )}
        </div>
      }
      backup="The current file is backed up before writing, and the result is verified by reading it back."
      confirmLabel="Apply"
      onConfirm={async () => {
        if (plan) await ipc.applyHostsChange({ ...request, expectedSha256: plan.sha256Before });
      }}
    />
  );
}
