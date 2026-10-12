import { ListChecks } from "lucide-react";
import { describeOperationType, isActive, needsRecovery, useOperations } from "../app/operations";
import { CancelOperationButton } from "./CancelOperationButton";
import { ErrorPanel } from "./ErrorPanel";
import { OperationStatusBadge } from "./StatusBadge";

/** Persistent list of long-running operations. Rendered by the shell, outside routed pages. */
export function OperationTray({ id, hidden }: { id: string; hidden?: boolean }) {
  const { operations, loadError } = useOperations();
  // Work needing attention first, then running, then history (each newest first).
  const ordered = [
    ...operations.filter(needsRecovery),
    ...operations.filter(isActive),
    ...operations.filter((op) => !isActive(op) && !needsRecovery(op)),
  ];

  return (
    <section
      id={id}
      hidden={hidden}
      aria-label="Operations"
      className="border-t border-border bg-surface-raised"
    >
      <h2 className="sr-only">Operations</h2>
      {loadError && (
        <div className="p-3">
          <ErrorPanel error={loadError} />
        </div>
      )}
      {ordered.length === 0 ? (
        !loadError && (
          <p className="flex items-center gap-2 px-4 py-3 text-sm text-fg-muted">
            <ListChecks aria-hidden className="size-4" />
            No operations yet. Installs, service changes and other long tasks will appear here.
          </p>
        )
      ) : (
        <ul className="max-h-48 divide-y divide-border overflow-y-auto">
          {ordered.map((op) => (
            <li key={op.operationId} className="flex flex-wrap items-center gap-3 px-4 py-2 text-sm">
              <OperationStatusBadge status={op.status} />
              <span className="font-medium">{describeOperationType(op.operationType)}</span>
              <span className="min-w-0 flex-1 truncate text-fg-muted">
                {needsRecovery(op)
                  ? "Interrupted when Devluma closed. Review before retrying."
                  : (op.safeMessage ?? op.phase ?? "")}
              </span>
              {isActive(op) && op.total !== undefined && op.current !== undefined && (
                <progress
                  className="w-32"
                  max={op.total}
                  value={op.current}
                  aria-label={`Progress for ${describeOperationType(op.operationType)}`}
                />
              )}
              {isActive(op) && (
                <CancelOperationButton
                  operationId={op.operationId}
                  label={`Cancel ${describeOperationType(op.operationType).toLowerCase()}`}
                />
              )}
              <span className="font-mono text-xs text-fg-muted">{op.operationId}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
