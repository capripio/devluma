import { useEffect, useState, type ReactNode } from "react";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { AppError, BACKEND_ERROR_CODES, toAppError, type ErrorCode } from "../../lib/ipc/errors";
import type { OperationProgress, SwitchPreview, SwitchRequest } from "../../types/dto";

interface RuntimeSwitchDialogProps {
  title: string;
  request: SwitchRequest;
  /** Optional control above the preview (e.g. a version picker). */
  selector?: ReactNode;
  confirmLabel: string;
  onClose: () => void;
  onApplied: () => void;
}

type PreviewState =
  { status: "pending" } | { status: "ready"; preview: SwitchPreview } | { status: "error"; error: AppError };

/** Turns a journaled operation that didn't succeed into an error the dialog can show. */
function operationError(op: OperationProgress): AppError {
  const code = (BACKEND_ERROR_CODES as readonly string[]).includes(op.errorCode ?? "")
    ? (op.errorCode as ErrorCode)
    : "INTERNAL";
  return new AppError({
    code,
    message: op.safeMessage ?? "The change could not be applied. Nothing was changed.",
    operationId: op.operationId,
    recoverable: true,
  });
}

/**
 * Preview → confirm → apply for a runtime selection (app_flow.md §5). The backend verifies
 * the target executables and applies in one transaction; a failure leaves everything as it was.
 */
export function RuntimeSwitchDialog({
  title,
  request,
  selector,
  confirmLabel,
  onClose,
  onApplied,
}: RuntimeSwitchDialogProps) {
  const requestKey = JSON.stringify(request);
  // Each result remembers which request it answers, so a stale preview is never shown and
  // "pending" is simply "no result for the current request yet".
  const [result, setResult] = useState<{ key: string; state: PreviewState } | null>(null);
  const state: PreviewState = result?.key === requestKey ? result.state : { status: "pending" };

  useEffect(() => {
    let active = true;
    ipc.previewRuntimeSwitch(JSON.parse(requestKey) as SwitchRequest).then(
      (preview) => active && setResult({ key: requestKey, state: { status: "ready", preview } }),
      (e: unknown) =>
        active && setResult({ key: requestKey, state: { status: "error", error: toAppError(e) } }),
    );
    return () => {
      active = false;
    };
  }, [requestKey]);

  const target =
    state.status === "ready"
      ? (state.preview.target?.installPath ?? "No version (nothing selected)")
      : "Checking…";

  return (
    <ConfirmChangeDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={title}
      target={target}
      confirmLabel={confirmLabel}
      confirmDisabled={state.status !== "ready" || state.preview.noOp}
      impact={
        <div className="flex flex-col gap-3">
          {selector}
          {state.status === "pending" && (
            <p role="status" className="text-fg-muted">
              Checking what would change…
            </p>
          )}
          {state.status === "error" && <ErrorPanel error={state.error} />}
          {state.status === "ready" && <PreviewSummary preview={state.preview} />}
        </div>
      }
      rollback="The executables are run and their version checked before anything changes. If that check or the change fails, the previous selection stays in effect."
      onConfirm={async () => {
        const op = await ipc.applyRuntimeSwitch(request);
        if (op.status !== "succeeded") throw operationError(op);
        onApplied();
      }}
    />
  );
}

function PreviewSummary({ preview }: { preview: SwitchPreview }) {
  if (preview.noOp) {
    return <p className="text-fg-muted">That's already the selected version. Nothing would change.</p>;
  }
  return (
    <>
      <ul aria-label="Changes" className="list-inside list-disc space-y-1">
        {preview.changes.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
      <div>
        <p className="font-medium">Not affected</p>
        <ul aria-label="Not affected" className="list-inside list-disc space-y-1 text-fg-muted">
          {preview.unchanged.map((u) => (
            <li key={u}>{u}</li>
          ))}
        </ul>
      </div>
    </>
  );
}
