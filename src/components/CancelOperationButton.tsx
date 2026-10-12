import { useState } from "react";
import { ipc } from "../lib/ipc";
import { toAppError, type AppError } from "../lib/ipc/errors";
import { Button } from "./Button";

export function CancelOperationButton({ operationId, label }: { operationId: string; label: string }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const cancel = () => {
    setPending(true);
    setError(null);
    ipc.cancelOperation(operationId).then(
      // Keep the button disabled: the operation reports `cancelled` once it has cleaned up.
      () => {},
      (e: unknown) => {
        setPending(false);
        setError(toAppError(e));
      },
    );
  };
  return (
    <div className="flex flex-col gap-1">
      <Button onClick={cancel} pending={pending} aria-label={label}>
        Cancel
      </Button>
      {error && <span className="text-xs text-danger">{error.message}</span>}
    </div>
  );
}
