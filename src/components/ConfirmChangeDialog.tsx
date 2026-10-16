import * as Dialog from "@radix-ui/react-dialog";
import { useRef, useState, type ReactNode } from "react";
import { toAppError, type AppError } from "../lib/ipc/errors";
import { Button } from "./Button";
import { ErrorPanel } from "./ErrorPanel";

interface ConfirmChangeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** Exact target, e.g. a full path. Shown in monospace and copyable via selection. */
  target: string;
  /** What will change and its consequence — not "Are you sure?" (docs/design-brief.md §6). */
  impact: ReactNode;
  backup?: ReactNode;
  rollback?: ReactNode;
  confirmLabel: string;
  destructive?: boolean;
  /** e.g. while a preview is still loading. */
  confirmDisabled?: boolean;
  /** Resolve to close; reject to keep the dialog open and show the error. */
  onConfirm: () => Promise<void>;
}

export function ConfirmChangeDialog({
  open,
  onOpenChange,
  title,
  target,
  impact,
  backup,
  rollback,
  confirmLabel,
  destructive = false,
  confirmDisabled = false,
  onConfirm,
}: ConfirmChangeDialogProps) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  // State updates are async, so a ref guards against double-submits within one tick.
  const inFlight = useRef(false);

  const confirm = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setError(null);
    try {
      await onConfirm();
      onOpenChange(false);
    } catch (e) {
      setError(toAppError(e));
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  };

  const handleOpenChange = (next: boolean) => {
    // Don't let Escape/overlay dismiss the dialog mid-operation.
    if (pending) return;
    if (!next) setError(null);
    onOpenChange(next);
  };

  return (
    <Dialog.Root open={open} onOpenChange={handleOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-[rgb(5_10_22/0.55)]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 flex max-h-[85vh] w-[min(36rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-md border border-border-strong bg-surface-raised p-5 shadow-[0_24px_64px_-12px_rgb(0_0_0/0.5)]">
          <Dialog.Title className="text-lg font-semibold">{title}</Dialog.Title>
          <Dialog.Description asChild>
            <div className="flex flex-col gap-3 text-sm">
              <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2">
                <dt className="text-fg-muted">Target</dt>
                <dd className="font-mono break-all select-all">{target}</dd>
                <dt className="text-fg-muted">Change</dt>
                <dd>{impact}</dd>
                {backup && (
                  <>
                    <dt className="text-fg-muted">Backup</dt>
                    <dd>{backup}</dd>
                  </>
                )}
                {rollback && (
                  <>
                    <dt className="text-fg-muted">Undo</dt>
                    <dd>{rollback}</dd>
                  </>
                )}
              </dl>
            </div>
          </Dialog.Description>
          {error && <ErrorPanel error={error} />}
          <div className="flex justify-end gap-2">
            <Dialog.Close asChild>
              <Button disabled={pending}>Cancel</Button>
            </Dialog.Close>
            <Button
              variant={destructive ? "danger" : "primary"}
              pending={pending}
              disabled={confirmDisabled}
              onClick={confirm}
            >
              {confirmLabel}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
