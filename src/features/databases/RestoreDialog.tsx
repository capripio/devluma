import { useEffect, useState } from "react";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { BackupRecord, DatabaseServiceView, RestorePlan, RestoreRequest } from "../../types/dto";

/**
 * Restore with a preview from the backend: what is created or replaced, and the safety net.
 * Replacing existing data needs an extra, explicit checkbox (app_flow.md §9).
 */
export function RestoreDialog({
  backup,
  services,
  onClose,
  onRestored,
}: {
  backup: BackupRecord;
  services: DatabaseServiceView[];
  onClose: () => void;
  onRestored: () => void;
}) {
  const [serviceId, setServiceId] = useState(
    services.find((s) => s.serviceId === backup.serviceInstanceId)?.serviceId ?? services[0]?.serviceId ?? "",
  );
  const [target, setTarget] = useState(backup.databaseName ?? "");
  // Previews and the overwrite confirmation belong to one set of inputs; changing an input
  // makes them stale (they're ignored until a fresh preview arrives).
  const inputs = `${backup.id}|${serviceId}|${target.trim()}`;
  const [preview, setPreview] = useState<{ inputs: string; plan?: RestorePlan; error?: AppError } | null>(
    null,
  );
  const [confirmedFor, setConfirmedFor] = useState<string | null>(null);
  const plan = preview?.inputs === inputs ? (preview.plan ?? null) : null;
  const previewError = preview?.inputs === inputs ? (preview.error ?? null) : null;
  const confirmed = confirmedFor === inputs;

  const request = (confirmOverwrite: boolean): RestoreRequest => ({
    backupId: backup.id,
    ...(serviceId ? { serviceId } : {}),
    ...(backup.kind === "logical" && target.trim() ? { targetDatabase: target.trim() } : {}),
    confirmOverwrite,
  });

  useEffect(() => {
    let current = true;
    const timer = setTimeout(() => {
      ipc.previewRestore(request(false)).then(
        (p) => current && setPreview({ inputs, plan: p }),
        (e: unknown) => current && setPreview({ inputs, error: toAppError(e) }),
      );
    }, 250);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- re-preview when the inputs change
  }, [inputs]);

  const ready = plan !== null && plan.blocked === null && (!plan.overwrites || confirmed);

  return (
    <ConfirmChangeDialog
      open
      destructive={plan?.overwrites ?? false}
      onOpenChange={(o) => !o && onClose()}
      title={backup.kind === "logical" ? `Restore ${backup.databaseName}?` : "Restore data folder snapshot?"}
      target={backup.path}
      impact={
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap gap-3">
            <label className="flex flex-col gap-1">
              <span className="text-fg-muted">Into service</span>
              <select
                value={serviceId}
                onChange={(e) => setServiceId(e.target.value)}
                className="rounded-md border border-border bg-surface px-2 py-1.5"
              >
                {services.map((s) => (
                  <option key={s.serviceId} value={s.serviceId}>
                    {s.name}
                  </option>
                ))}
              </select>
            </label>
            {backup.kind === "logical" && (
              <label className="flex flex-col gap-1">
                <span className="text-fg-muted">Into database</span>
                <input
                  value={target}
                  onChange={(e) => setTarget(e.target.value)}
                  className="w-48 rounded-md border border-border bg-surface px-2 py-1.5 font-mono"
                />
              </label>
            )}
          </div>
          {!plan && !previewError && <p role="status">Checking what this restore would change…</p>}
          {previewError && <ErrorPanel error={previewError} />}
          {plan && (
            <ul aria-label="Restore effects" className="list-inside list-disc space-y-1">
              {plan.effects.map((e) => (
                <li key={e}>{e}</li>
              ))}
            </ul>
          )}
          {plan?.blocked && <p className="text-warning">{plan.blocked}</p>}
          {plan?.overwrites && !plan.blocked && (
            <label className="flex items-start gap-2 font-medium">
              <input
                type="checkbox"
                checked={confirmed}
                onChange={(e) => setConfirmedFor(e.target.checked ? inputs : null)}
              />
              I understand this replaces the existing data described above.
            </label>
          )}
        </div>
      }
      rollback={
        backup.kind === "logical"
          ? "If the restore fails, the database is put back from the automatic backup taken just before."
          : "The current data folder is kept next to it; move it back to undo."
      }
      confirmLabel="Restore"
      confirmDisabled={!ready}
      onConfirm={async () => {
        await ipc.restoreDatabase(request(plan?.overwrites ?? false));
        onRestored();
      }}
    />
  );
}
