import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { DiffViewer } from "../../components/DiffViewer";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ConfigApplied, ConfigChange, ConfigTarget } from "../../types/dto";

interface SaveConfigDialogProps {
  target: ConfigTarget;
  change: ConfigChange;
  title: string;
  /** Plain-language list of what changes (settings form); omitted for the editor. */
  summary?: string[];
  onClose: () => void;
  onApplied: (result: ConfigApplied) => void;
  /** Called with the tool's message when the check rejects the change (the dialog stays open). */
  onFailed?: (message: string) => void;
  /** Settings Devluma keeps itself (no file of the user's is replaced). */
  managed?: boolean;
}

/**
 * Preview → confirm → apply. The backend backs up, writes, runs the tool's own check and
 * rolls back on failure; a rejected change keeps the dialog open with the tool's message.
 */
export function SaveConfigDialog({
  target,
  change,
  title,
  summary,
  onClose,
  onApplied,
  onFailed,
  managed = false,
}: SaveConfigDialogProps) {
  const preview = useAsync(() => ipc.previewConfigChange(target, change));
  const p = preview.status === "success" ? preview.data : null;
  const unchanged = p !== null && p.diff.trim() === "";
  return (
    <ConfirmChangeDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={title}
      target={p?.path ?? "…"}
      confirmLabel="Save and check"
      confirmDisabled={p === null || unchanged}
      impact={
        <div className="flex flex-col gap-3">
          {summary && summary.length > 0 && (
            <ul className="list-inside list-disc space-y-1">
              {summary.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          )}
          {preview.status === "pending" && <LoadingState label="Preparing the preview…" rows={3} />}
          {preview.status === "error" && <ErrorPanel error={preview.error} onRetry={preview.reload} />}
          {p &&
            (unchanged ? (
              <p className="text-fg-muted">Nothing would change.</p>
            ) : (
              <DiffViewer diff={p.diff} label="Exact change" />
            ))}
          {p && <p className="text-fg-muted">{p.check}</p>}
        </div>
      }
      backup={
        managed
          ? "Devluma keeps the previous settings and puts them back if the check fails."
          : "The current file is copied to Devluma's backups before it's replaced."
      }
      rollback={
        p?.applies === "reload"
          ? "If the check fails, nothing is changed. A running server reloads only after the check passes."
          : "If the check fails, nothing is changed. A running service keeps its current settings until it restarts."
      }
      onConfirm={async () => {
        try {
          onApplied(await ipc.applyConfigChange(target, change));
        } catch (e) {
          onFailed?.(toAppError(e).message);
          throw e;
        }
      }}
    />
  );
}
