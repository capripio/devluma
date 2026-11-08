import { useState } from "react";
import { Link } from "react-router";
import { useOperations } from "../../app/operations";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { OperationStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { DiagnosticsPreview } from "../../types/dto";

export function LogsPage() {
  return (
    <>
      <PageHeader
        title="Logs & Diagnostics"
        description="What Devluma did, service logs and a diagnostics export."
      />
      <OperationHistory />
      <ServiceLogs />
      <DiagnosticExport />
    </>
  );
}

function OperationHistory() {
  const { operations } = useOperations();
  return (
    <section aria-labelledby="ops-heading" className="flex flex-col gap-2">
      <h2 id="ops-heading" className="text-sm font-semibold">
        Recent operations
      </h2>
      {operations.length === 0 ? (
        <p className="text-sm text-fg-muted">Nothing has run yet.</p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full text-left text-sm">
            <thead className="bg-surface-raised text-fg-muted">
              <tr>
                {["Started", "Operation", "Status", "Details"].map((h) => (
                  <th key={h} scope="col" className="px-3 py-2 font-medium">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {operations.map((op) => (
                <tr key={op.operationId} className="align-top">
                  <td className="px-3 py-2 whitespace-nowrap">{new Date(op.startedAt).toLocaleString()}</td>
                  <td className="px-3 py-2 font-mono text-xs">{op.operationType}</td>
                  <td className="px-3 py-2">
                    <OperationStatusBadge status={op.status} />
                  </td>
                  <td className="px-3 py-2 text-xs">{op.safeMessage ?? op.phase ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function ServiceLogs() {
  const services = useAsync(ipc.listServices);
  return (
    <section aria-labelledby="service-logs-heading" className="flex flex-col gap-2 text-sm">
      <h2 id="service-logs-heading" className="font-semibold">
        Service logs
      </h2>
      {services.status === "success" && services.data.length === 0 && (
        <p className="text-fg-muted">No services yet.</p>
      )}
      {services.status === "success" && services.data.length > 0 && (
        <ul className="flex flex-wrap gap-3">
          {services.data.map((s) => (
            <li key={s.id}>
              <Link to={`/services/${s.id}`} className="text-accent hover:underline">
                {s.instanceName} ({s.packageName})
              </Link>
            </li>
          ))}
        </ul>
      )}
      <p className="text-fg-muted">Each service&apos;s Logs tab shows its recent output, with secrets redacted.</p>
    </section>
  );
}

function DiagnosticExport() {
  const [mask, setMask] = useState(true);
  const [preview, setPreview] = useState<DiagnosticsPreview | null>(null);
  const [busy, setBusy] = useState<"preview" | "export" | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const run = async (kind: "preview" | "export") => {
    setBusy(kind);
    setError(null);
    try {
      if (kind === "preview") {
        setSaved(null);
        setPreview(await ipc.previewDiagnostics(mask));
      } else {
        setSaved(await ipc.exportDiagnostics(mask));
      }
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section aria-labelledby="diag-heading" className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm">
      <h2 id="diag-heading" className="font-semibold">
        Diagnostic export
      </h2>
      <p className="text-fg-muted">
        Review everything first: the export contains exactly the sections shown below, nothing else. Secrets are
        redacted.
      </p>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={mask}
          onChange={(e) => {
            setMask(e.target.checked);
            setPreview(null);
            setSaved(null);
          }}
        />
        Hide my Windows user folder in paths
      </label>
      <div className="flex flex-wrap gap-2">
        <Button pending={busy === "preview"} disabled={busy !== null} onClick={() => void run("preview")}>
          {preview ? "Refresh preview" : "Preview export"}
        </Button>
        <Button
          variant="primary"
          pending={busy === "export"}
          disabled={!preview || busy !== null}
          onClick={() => void run("export")}
        >
          Export diagnostics
        </Button>
      </div>
      {error && <ErrorPanel error={error} />}
      {saved && (
        <div role="status" className="flex flex-wrap items-center gap-2">
          <span>
            Saved to <span className="font-mono break-all">{saved}</span>
          </span>
          <Button onClick={() => void ipc.openDiagnosticsFolder()}>Open folder</Button>
        </div>
      )}
      {preview && (
        <div className="flex flex-col gap-2">
          <p>
            <span className="font-medium">Never included:</span> {preview.neverIncluded.join("; ")}.
          </p>
          {preview.sections.map((s) => (
            <details key={s.file} className="rounded-md border border-border">
              <summary className="cursor-pointer px-3 py-2">
                {s.title} <span className="font-mono text-xs text-fg-muted">({s.file})</span>
              </summary>
              <pre className="max-h-72 overflow-auto px-3 py-2 font-mono text-xs whitespace-pre-wrap">
                {s.content || "(empty)"}
              </pre>
            </details>
          ))}
        </div>
      )}
    </section>
  );
}
