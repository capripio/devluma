import { useState } from "react";
import { Button } from "../../components/Button";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { ServiceView } from "../../types/dto";
import { portConflictText } from "./portConflicts";

type Action = "start" | "stop" | "restart";

const CALLS: Record<Action, (id: string) => Promise<unknown>> = {
  start: ipc.startService,
  stop: ipc.stopService,
  restart: ipc.restartService,
};

/**
 * Start/Stop/Restart for one service. Buttons follow the state the backend reports; while a
 * request is being accepted they're disabled to prevent duplicate submissions.
 */
export function ServiceActions({ service }: { service: ServiceView }) {
  const [pending, setPending] = useState<Action | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const s = service.observedState;
  const transitioning = s === "starting" || s === "stopping";
  const canStart = s === "stopped" || s === "failed";
  const canStop = s === "running" || s === "degraded" || s === "starting";

  const run = (action: Action) => {
    setPending(action);
    setError(null);
    CALLS[action](service.id).then(
      () => setPending(null),
      (e: unknown) => {
        setPending(null);
        setError(toAppError(e));
      },
    );
  };

  const label = (verb: string) => `${verb} ${service.instanceName}`;
  const conflict = portConflictText(service);
  return (
    <div className="flex flex-col gap-1">
      {conflict && (
        <p role="status" className="text-sm text-warning">
          {conflict} This service can&apos;t start until that program stops or you choose another port.
          Devluma never stops other programs.
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {canStart && (
          <Button
            variant="primary"
            pending={pending === "start"}
            disabled={pending !== null}
            onClick={() => run("start")}
            aria-label={label("Start")}
          >
            Start
          </Button>
        )}
        {canStop && (
          <Button
            pending={pending === "stop"}
            disabled={pending !== null || s === "starting"}
            onClick={() => run("stop")}
            aria-label={label("Stop")}
          >
            Stop
          </Button>
        )}
        {(s === "running" || s === "degraded") && (
          <Button
            // A degraded service's next step is usually a restart (Stop stays secondary).
            variant={s === "degraded" ? "primary" : "secondary"}
            pending={pending === "restart"}
            disabled={pending !== null}
            onClick={() => run("restart")}
            aria-label={label("Restart")}
          >
            Restart
          </Button>
        )}
        {s === "running" && service.adminPort !== null && (
          <Button
            onClick={() => {
              setError(null);
              ipc.openServiceUi(service.id).catch((e: unknown) => setError(toAppError(e)));
            }}
            aria-label={`Open the web interface of ${service.instanceName}`}
          >
            Open web interface
          </Button>
        )}
        {transitioning && <span className="text-xs text-fg-muted">Waiting for the service…</span>}
      </div>
      {error && <span className="text-xs text-danger">{error.message}</span>}
    </div>
  );
}
