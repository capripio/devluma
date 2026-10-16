import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { events, ipc } from "../lib/ipc";
import { toAppError, type AppError } from "../lib/ipc/errors";
import type { OperationProgress } from "../types/dto";

// Operations live above the router so they stay visible across navigation (docs/design-brief.md §6).
// Seeded from the persisted journal (so interrupted work is visible after a restart) and
// kept current by `operation://progress` events.

interface OperationsValue {
  /** Newest first. */
  operations: OperationProgress[];
  loadError: AppError | null;
}

const OperationsContext = createContext<OperationsValue | null>(null);

export function OperationsProvider({ children }: { children: ReactNode }) {
  const [operations, setOperations] = useState<Map<string, OperationProgress>>(new Map());
  const [loadError, setLoadError] = useState<AppError | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    // Subscribe first so nothing emitted during the initial load is missed.
    events
      .onOperationProgress((progress) => {
        setOperations((prev) => new Map(prev).set(progress.operationId, progress));
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Listening is best-effort; the journal load below still shows persisted state.
      });

    ipc.listOperations().then(
      (persisted) => {
        if (disposed) return;
        setOperations((prev) => {
          const next = new Map(prev);
          // A live event that arrived during the load is newer than the persisted row.
          for (const op of persisted) if (!next.has(op.operationId)) next.set(op.operationId, op);
          return next;
        });
      },
      (error: unknown) => {
        if (!disposed) setLoadError(toAppError(error));
      },
    );

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const sorted = [...operations.values()].sort((a, b) =>
    a.startedAt < b.startedAt ? 1 : a.startedAt > b.startedAt ? -1 : 0,
  );
  return <OperationsContext value={{ operations: sorted, loadError }}>{children}</OperationsContext>;
}

export function useOperations(): OperationsValue {
  const value = useContext(OperationsContext);
  if (!value) throw new Error("useOperations must be used inside <OperationsProvider>");
  return value;
}

/**
 * Calls `onSettled` whenever an operation matching `filter` reaches a final state
 * (e.g. to refetch data an install changed). Doesn't fire for operations already finished
 * when the component mounted.
 */
export function useOnOperationsSettled(filter: (op: OperationProgress) => boolean, onSettled: () => void) {
  const { operations } = useOperations();
  const signature = operations
    .filter((op) => filter(op) && !isActive(op))
    .map((op) => `${op.operationId}:${op.status}`)
    .sort()
    .join(",");
  const initial = useRef(signature);
  const callback = useRef(onSettled);
  useEffect(() => {
    callback.current = onSettled;
  });
  useEffect(() => {
    if (signature !== initial.current) {
      initial.current = signature;
      callback.current();
    }
  }, [signature]);
}

export function isActive(op: OperationProgress): boolean {
  return op.status === "queued" || op.status === "running";
}

export function needsRecovery(op: OperationProgress): boolean {
  return op.status === "needs_recovery";
}

/** "install_package" → "Install package". */
export function describeOperationType(type: string): string {
  const words = type.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}
