import { useEffect, useState } from "react";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { UninstallPlan } from "../../types/dto";

/** Uninstall preview from the backend: what's deleted, what's cleared, what's kept. */
export function UninstallDialog({
  installationId,
  label,
  onClose,
  onDone,
}: {
  installationId: string;
  label: string;
  onClose: () => void;
  onDone: () => void;
}) {
  const [plan, setPlan] = useState<UninstallPlan | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  useEffect(() => {
    let current = true;
    ipc.previewUninstall(installationId).then(
      (p) => current && setPlan(p),
      (e: unknown) => current && setError(toAppError(e)),
    );
    return () => {
      current = false;
    };
  }, [installationId]);

  return (
    <ConfirmChangeDialog
      open
      destructive
      onOpenChange={(o) => !o && onClose()}
      title={`Uninstall ${label}?`}
      target={plan?.installPath ?? "…"}
      impact={
        <div className="flex flex-col gap-2">
          {!plan && !error && <p role="status">Checking what uses this version…</p>}
          {error && <ErrorPanel error={error} />}
          {plan && !plan.canUninstall && (
            <div className="text-warning">
              <p>This version is still in use:</p>
              <ul className="list-inside list-disc">
                {plan.blockingServices.map((s) => (
                  <li key={s}>Service {s} — remove it first (its data folder is kept)</li>
                ))}
                {plan.blockingSites.map((s) => (
                  <li key={s}>Site {s} uses this PHP version — change or remove the site first</li>
                ))}
              </ul>
            </div>
          )}
          {plan?.canUninstall && (
            <ul className="list-inside list-disc space-y-1">
              <li>The folder above is deleted.</li>
              {plan.clearsGlobalDefault && (
                <li>It stops being the default version; no other version becomes default.</li>
              )}
              {plan.pinsCleared.length > 0 && (
                <li>
                  These projects lose their pin and follow the default again: {plan.pinsCleared.join(", ")}.
                </li>
              )}
              {plan.backendsRemoved > 0 && <li>Its stopped PHP backend is removed.</li>}
              <li>Kept: {plan.kept.join(", ")}.</li>
            </ul>
          )}
        </div>
      }
      rollback="Install the version again from this page to get it back. Selections cleared above have to be set again."
      confirmLabel="Uninstall"
      confirmDisabled={!plan?.canUninstall}
      onConfirm={async () => {
        await ipc.uninstallPackage(installationId);
        onDone();
      }}
    />
  );
}
