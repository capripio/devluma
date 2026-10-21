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
        description="Local domains Devluma points at this computer. Only its own lines are touched."
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
  const [change, setChange] = useState<{ title: string; request: HostsChangeRequest } | null>(null);
  const sites = useAsync(ipc.listSites);
  const siteFor = (domain: string) =>
    sites.status === "success" ? sites.data.find((s) => s.domain === domain) : undefined;
  return (
    <>
      <p className="flex items-center gap-1 font-mono text-xs break-all text-fg-muted">
        {view.path}
        <CopyButton value={view.path} label="hosts file path" />
      </p>
      {view.strayTags.length > 0 && (
        <section
          aria-label="Hosts file problem"
          className="flex gap-3 rounded-md border border-warning/40 bg-warning-subtle p-4 text-sm"
        >
          <TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning" />
          <div className="flex flex-col gap-1">
            <p>
              {view.strayTags.length === 1 ? "A line carries" : `${view.strayTags.length} lines carry`}{" "}
              Devluma&apos;s tag but point{view.strayTags.length === 1 ? "s" : ""} away from this computer, so
              Devluma leaves {view.strayTags.length === 1 ? "it" : "them"} alone:
            </p>
            <ul className="font-mono text-xs">
              {view.strayTags.map((t) => (
                <li key={t.line}>
                  line {t.line}: {t.ip} {t.hostnames.join(" ")}
                </li>
              ))}
            </ul>
          </div>
        </section>
      )}
      <div className="overflow-x-auto rounded-md border border-border">
        <table className="w-full text-left text-sm">
          <caption className="sr-only">Devluma&apos;s entries in the hosts file</caption>
          <thead className="bg-surface-raised">
            <tr>
              {["Domain", "Address", "Used by", ""].map((h, i) => (
                <th key={i} scope="col" className="px-3 py-2 font-semibold">
                  {h || <span className="sr-only">Actions</span>}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {view.entries.length === 0 && (
              <tr>
                <td colSpan={4} className="px-3 py-3 text-fg-muted">
                  No Devluma entries yet. Adding a site creates one.
                </td>
              </tr>
            )}
            {view.entries.flatMap((e) =>
              e.hostnames.map((domain) => {
                const site = siteFor(domain);
                return (
                  <tr key={`${e.line}:${domain}`}>
                    <th scope="row" className="px-3 py-2 font-mono font-normal">
                      {domain}
                    </th>
                    <td className="px-3 py-2 font-mono">{e.ip}</td>
                    <td className="px-3 py-2">
                      {site ? site.projectName : <span className="text-fg-muted">No site</span>}
                    </td>
                    <td className="px-3 py-2">
                      <Button
                        onClick={() =>
                          setChange({
                            title: `Remove ${domain} from the hosts file?`,
                            request: { removeDomains: [domain] },
                          })
                        }
                        aria-label={`Remove ${domain}`}
                      >
                        Remove
                      </Button>
                    </td>
                  </tr>
                );
              }),
            )}
          </tbody>
        </table>
      </div>
      <p className="text-xs text-fg-muted">
        Devluma marks its lines with <span className="font-mono"># managed by devluma</span> and leaves the{" "}
        {view.otherCount === 1 ? "other entry" : `other ${view.otherCount} entries`} in this file alone.
      </p>

      <section aria-labelledby="backups-heading" className="flex flex-col gap-2">
        <h2 id="backups-heading" className="text-sm font-semibold">
          Backups
        </h2>
        {view.backups.length === 0 ? (
          <p className="text-sm text-fg-muted">Devluma hasn&apos;t changed the hosts file yet.</p>
        ) : (
          <ul className="divide-y divide-border rounded-md border border-border text-sm">
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
