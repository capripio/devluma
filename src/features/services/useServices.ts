import { useEffect, useState } from "react";
import { useOnOperationsSettled } from "../../app/operations";
import { events, ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import type { ServiceStatus, ServiceView } from "../../types/dto";

/** Operation types that change the service list or a service's state. */
const SERVICE_OPERATIONS = new Set([
  "create_service",
  "start_service",
  "stop_service",
  "restart_service",
  "service_exited",
]);

/**
 * Services from the backend, kept current by `service://state` events (never optimistic:
 * the UI only shows states the backend reported).
 */
export function useServices() {
  const list = useAsync(ipc.listServices);
  const [live, setLive] = useState<Map<string, ServiceStatus>>(new Map());

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    events
      .onServiceState((s) => setLive((prev) => new Map(prev).set(s.serviceId, s)))
      .then((fn) => (disposed ? fn() : (unlisten = fn)))
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useOnOperationsSettled((op) => SERVICE_OPERATIONS.has(op.operationType), list.refresh);

  const merge = (s: ServiceView): ServiceView => {
    const l = live.get(s.id);
    return l
      ? {
          ...s,
          observedState: l.observedState,
          healthStatus: l.healthStatus,
          pid: l.pid,
          lastExitCode: l.lastExitCode,
          lastErrorCode: l.lastErrorCode,
        }
      : s;
  };

  return list.status === "success" ? { ...list, data: list.data.map(merge) } : list;
}
