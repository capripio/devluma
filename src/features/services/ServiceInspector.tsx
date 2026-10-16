import type { ReactNode } from "react";
import { describeOperationType, useOperations } from "../../app/operations";
import { CopyButton } from "../../components/CopyButton";
import { OperationStatusBadge, ServiceStatusBadge } from "../../components/StatusBadge";
import type { ServiceView } from "../../types/dto";
import { ServiceActions } from "./ServiceActions";

const ROLE_LABELS: Record<NonNullable<ServiceView["role"]>, string> = {
  web_server: "Web server",
  database: "Database",
  fast_cgi: "PHP (FastCGI)",
  mail: "Mail",
};

export const HEALTH_LABELS: Record<string, string> = {
  healthy: "Accepting connections",
  unreachable: "Not accepting connections",
  unknown: "Not checked yet",
};

/** One sentence on what the state means, from what the backend reported. */
export function explainService(s: ServiceView): string {
  switch (s.observedState) {
    case "running":
      return s.healthStatus === "healthy"
        ? `Running and accepting connections on 127.0.0.1:${s.port ?? "?"}.`
        : "Running.";
    case "degraded":
      return `Running, but the health check reports “${HEALTH_LABELS[s.healthStatus] ?? s.healthStatus}”.`;
    case "failed":
      return s.lastExitCode !== null
        ? `Stopped unexpectedly with exit code ${s.lastExitCode}.`
        : "Stopped unexpectedly.";
    case "starting":
      return "Starting. Waiting for the service to accept connections.";
    case "stopping":
      return "Stopping.";
    case "stopped":
      return "Stopped.";
    case "not_installed":
      return "Its package is not installed.";
    default:
      return "Devluma couldn't confirm this service's state.";
  }
}

/** Detail pane body for one service: state, facts, actions, its recent operations. */
export function ServiceInspector({ service: s, children }: { service: ServiceView; children?: ReactNode }) {
  const { operations } = useOperations();
  const recent = operations.filter((o) => o.targetId === `service:${s.id}`).slice(0, 4);
  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-1.5">
        <h2 className="font-display text-xl font-semibold">
          {s.packageName} <span className="font-mono text-base font-normal text-fg-muted">{s.version}</span>
        </h2>
        <div className="flex flex-wrap items-center gap-2">
          <ServiceStatusBadge state={s.observedState} />
          <span className="text-xs text-fg-muted">
            {s.role ? ROLE_LABELS[s.role] : s.serviceType} · {s.instanceName}
          </span>
        </div>
      </header>
      <p className="text-sm text-pretty">{explainService(s)}</p>
      <ServiceActions service={s} />
      <dl className="grid grid-cols-[6rem_1fr] gap-x-3 gap-y-2 text-sm">
        <dt className="text-fg-muted">Port</dt>
        <dd className="font-mono">
          {s.port ?? "—"}
          {s.adminPort !== null && <span className="text-fg-muted"> · UI {s.adminPort}</span>}
        </dd>
        <dt className="text-fg-muted">Process</dt>
        <dd className="font-mono">{s.pid ? `PID ${s.pid}` : "Not running"}</dd>
        <dt className="text-fg-muted">Health</dt>
        <dd>{HEALTH_LABELS[s.healthStatus] ?? s.healthStatus}</dd>
        {s.configPath && <PathRow label="Config" path={s.configPath} />}
        {s.dataPath && <PathRow label="Data" path={s.dataPath} />}
      </dl>
      {children}
      {recent.length > 0 && (
        <section aria-label={`Recent operations for ${s.instanceName}`} className="flex flex-col gap-2">
          <h3 className="label-caps text-fg-muted">Recent operations</h3>
          <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
            {recent.map((op) => (
              <li key={op.operationId} className="flex flex-col gap-1 px-3 py-2 text-sm">
                <span className="flex items-center justify-between gap-2">
                  {describeOperationType(op.operationType)}
                  <OperationStatusBadge status={op.status} />
                </span>
                {op.safeMessage && <span className="text-xs text-fg-muted">{op.safeMessage}</span>}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function PathRow({ label, path }: { label: string; path: string }) {
  return (
    <>
      <dt className="text-fg-muted">{label}</dt>
      <dd className="flex min-w-0 items-start gap-1">
        <span className="min-w-0 font-mono text-xs break-all select-all">{path}</span>
        <CopyButton value={path} label={`${label.toLowerCase()} path`} />
      </dd>
    </>
  );
}
