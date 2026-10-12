import { Globe } from "lucide-react";
import { useState } from "react";
import { useOnOperationsSettled, useOperations } from "../../app/operations";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { SiteView } from "../../types/dto";
import { SharedAddress, ShareSite } from "../tunnels/ShareSite";
import { SiteForm } from "./SiteForm";

const SITE_OPERATIONS = new Set(["apply_site", "remove_site", "apply_hosts"]);

export function SitesPage() {
  const sites = useAsync(ipc.listSites);
  const [form, setForm] = useState<{ existing?: SiteView } | null>(null);
  useOnOperationsSettled((op) => SITE_OPERATIONS.has(op.operationType), sites.refresh);

  const addButton = (
    <Button variant="primary" onClick={() => setForm({})} disabled={form !== null}>
      Add site
    </Button>
  );
  return (
    <>
      <PageHeader
        title="Sites & Domains"
        description="Project folders served by Nginx at local domains."
        actions={addButton}
      />
      {form && <SiteForm existing={form.existing} onDone={() => setForm(null)} />}
      {sites.status === "pending" && (
        <p role="status" className="text-sm text-fg-muted">
          Loading sites…
        </p>
      )}
      {sites.status === "error" && <ErrorPanel error={sites.error} onRetry={sites.reload} />}
      {sites.status === "success" &&
        (sites.data.length === 0 ? (
          !form && (
            <EmptyState
              icon={Globe}
              title="No sites yet"
              description="Serve a project at a local domain like my-app.test. You'll see the exact configuration and hosts-file change before anything is applied."
              action={addButton}
            />
          )
        ) : (
          <SitesTable sites={sites.data} onEdit={(s) => setForm({ existing: s })} />
        ))}
    </>
  );
}

function SitesTable({ sites, onEdit }: { sites: SiteView[]; onEdit: (s: SiteView) => void }) {
  const { operations } = useOperations();
  const tunnels = useAsync(ipc.listTunnels);
  const tunnelFor = (s: SiteView) =>
    tunnels.status === "success" ? tunnels.data.find((t) => t.siteId === s.id) : undefined;
  const [removing, setRemoving] = useState<SiteView | null>(null);
  const [notes, setNotes] = useState<Record<string, string>>({});
  const note = (id: string, text: string) => setNotes((n) => ({ ...n, [id]: text }));

  const check = (s: SiteView) => {
    note(s.id, "Checking…");
    ipc.checkSite(s.id).then(
      (r) => note(s.id, r.message),
      (e: unknown) => note(s.id, toAppError(e).message),
    );
  };
  const open = (s: SiteView) => {
    ipc.openSite(s.id).then(
      () => note(s.id, `Opened ${s.url} in your browser.`),
      (e: unknown) => note(s.id, toAppError(e).message),
    );
  };
  const lastFailure = (s: SiteView) => {
    const op = operations.find((o) => o.targetId === s.id && o.operationType !== "remove_site");
    return op?.status === "failed" ? op.safeMessage : undefined;
  };

  return (
    <>
      <div className="overflow-x-auto rounded-lg border border-border">
        <table className="w-full text-left text-sm">
          <thead className="bg-surface-raised text-fg-muted">
            <tr>
              {["Site", "Project", "Server", "PHP", "Hosts file", "Actions"].map((h) => (
                <th key={h} scope="col" className="px-3 py-2 font-medium">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {sites.map((s) => {
              const failure = lastFailure(s);
              return (
                <tr key={s.id} className="align-top">
                  <td className="px-3 py-2">
                    <div className="font-medium">{s.domain}</div>
                    <div className="font-mono text-xs text-fg-muted">{s.url}</div>
                    {s.httpsUrl && <div className="font-mono text-xs text-fg-muted">{s.httpsUrl}</div>}
                    <SharedAddress tunnel={tunnelFor(s)} />
                    {!s.configPresent && (
                      <p className="text-xs text-danger">Configuration file is missing.</p>
                    )}
                    {failure && <p className="mt-1 max-w-md text-xs text-danger">{failure}</p>}
                    {notes[s.id] && (
                      <p aria-live="polite" className="mt-1 text-xs text-fg-muted">
                        {notes[s.id]}
                      </p>
                    )}
                  </td>
                  <td className="px-3 py-2">{s.projectName}</td>
                  <td className="px-3 py-2">
                    {s.webServerName}
                    {!s.webServerRunning && <div className="text-xs text-warning">Not running</div>}
                  </td>
                  <td className="px-3 py-2">
                    {s.phpVersion ?? "—"}
                    {s.phpOutdated && (
                      <div className="text-xs text-warning">Project PHP changed — re-apply to switch</div>
                    )}
                  </td>
                  <td className="px-3 py-2">{s.hostsManaged ? "Managed" : "Not added"}</td>
                  <td className="px-3 py-2">
                    <div className="flex flex-wrap gap-2">
                      <Button onClick={() => open(s)} aria-label={`Open ${s.domain}`}>
                        Open
                      </Button>
                      <Button onClick={() => check(s)} aria-label={`Check ${s.domain}`}>
                        Check
                      </Button>
                      <Button onClick={() => onEdit(s)} aria-label={`Re-apply ${s.domain}`}>
                        Re-apply
                      </Button>
                      <ShareSite site={s} tunnel={tunnelFor(s)} onChanged={tunnels.refresh} />
                      <Button onClick={() => setRemoving(s)} aria-label={`Remove ${s.domain}`}>
                        Remove
                      </Button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {removing && (
        <ConfirmChangeDialog
          open
          destructive
          onOpenChange={(o) => !o && setRemoving(null)}
          title={`Remove ${removing.domain}?`}
          target={removing.configPath}
          impact={
            <>
              Deletes this site&apos;s web server configuration
              {removing.hostsManaged
                ? " and its hosts-file entry (Windows may ask for administrator approval)"
                : ""}
              . The project folder isn&apos;t touched.
            </>
          }
          backup="The configuration and hosts file are backed up before they change."
          confirmLabel="Remove site"
          onConfirm={async () => {
            await ipc.removeSite(removing.id);
          }}
        />
      )}
    </>
  );
}
