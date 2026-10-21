import type { ServiceView } from "../../types/dto";

const LIVE = new Set(["starting", "running", "degraded", "stopping"]);

/**
 * "Port 80 is in use by httpd.exe (PID 1234)." for a service that isn't running and whose
 * port another program holds (from `list_services`); `null` otherwise.
 */
export function portConflictText(s: ServiceView): string | null {
  if (LIVE.has(s.observedState) || s.portConflicts.length === 0) return null;
  return s.portConflicts.map((c) => `Port ${c.port} is in use by ${c.usedBy}.`).join(" ");
}
