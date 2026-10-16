import * as Tabs from "@radix-ui/react-tabs";
import { ArrowLeft } from "lucide-react";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { PortsForm } from "./PortsForm";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ServiceStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import type { ServiceView } from "../../types/dto";
import { ServiceActions } from "./ServiceActions";
import { explainService } from "./ServiceInspector";
import { useServices } from "./useServices";
import { LoadingState } from "../../components/LoadingState";

/** Right-hand pane of the Services page for one service (the route stays deep-linkable). */
export function ServiceDetail() {
  const { serviceId = "" } = useParams();
  const services = useServices();
  const back = (
    <Link
      to="/services"
      className="inline-flex items-center gap-1 self-start text-sm text-accent hover:underline wide:hidden"
    >
      <ArrowLeft aria-hidden className="size-4" />
      All services
    </Link>
  );
  if (services.status === "pending") {
    return (
      <>
        {back}
        <LoadingState label="Loading service…" shape="panel" rows={4} />
      </>
    );
  }
  if (services.status === "error") {
    return (
      <>
        {back}
        <ErrorPanel error={services.error} onRetry={services.reload} />
      </>
    );
  }
  const svc = services.data.find((s) => s.id === serviceId);
  if (!svc) {
    return (
      <>
        {back}
        <PageHeader title="Service not found" description="It may have been removed." />
      </>
    );
  }
  return (
    <>
      {back}
      <PageHeader title={svc.instanceName} description={`${svc.packageName} ${svc.version}`} />
      <div className="flex flex-wrap items-center gap-3">
        <ServiceStatusBadge state={svc.observedState} />
        <span className="text-sm text-pretty text-fg-muted">{explainService(svc)}</span>
      </div>
      <ServiceActions service={svc} />
      <ServiceTabs service={svc} />
    </>
  );
}

const tabClass =
  "border-b-2 border-transparent px-3 py-2 text-sm text-fg-muted transition-colors hover:text-fg data-[state=active]:text-fg data-[state=active]:border-accent data-[state=active]:font-medium";

function ServiceTabs({ service: s }: { service: ServiceView }) {
  return (
    <Tabs.Root defaultValue="overview" className="flex flex-col gap-4">
      <Tabs.List aria-label="Service details" className="flex gap-1 border-b border-border">
        <Tabs.Trigger value="overview" className={tabClass}>
          Overview
        </Tabs.Trigger>
        <Tabs.Trigger value="config" className={tabClass}>
          Configuration
        </Tabs.Trigger>
        <Tabs.Trigger value="logs" className={tabClass}>
          Logs
        </Tabs.Trigger>
        <Tabs.Trigger value="deps" className={tabClass}>
          Dependencies
        </Tabs.Trigger>
      </Tabs.List>
      <Tabs.Content value="overview">
        <Overview service={s} />
      </Tabs.Content>
      <Tabs.Content value="config">
        <Configuration serviceId={s.id} />
      </Tabs.Content>
      <Tabs.Content value="logs">
        <Logs serviceId={s.id} />
      </Tabs.Content>
      <Tabs.Content value="deps">
        <p className="text-sm text-fg-muted">This service has no dependencies on other services.</p>
      </Tabs.Content>
    </Tabs.Root>
  );
}

function Overview({ service: s }: { service: ServiceView }) {
  const navigate = useNavigate();
  const [removing, setRemoving] = useState(false);
  const isDb = s.role === "database";
  const live = ["starting", "running", "degraded", "stopping"].includes(s.observedState);
  return (
    <div className="flex flex-col gap-4 text-sm">
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
        <dt className="text-fg-muted">Address</dt>
        <dd className="font-mono">127.0.0.1:{s.port}</dd>
        <dt className="text-fg-muted">Process</dt>
        <dd className="font-mono">{s.pid ? `PID ${s.pid}` : "Not running"}</dd>
        {s.lastExitCode !== null && (
          <>
            <dt className="text-fg-muted">Last exit code</dt>
            <dd className="font-mono">{s.lastExitCode}</dd>
          </>
        )}
        {s.lastErrorCode && (
          <>
            <dt className="text-fg-muted">Last error</dt>
            <dd className="font-mono">{s.lastErrorCode}</dd>
          </>
        )}
        {s.dataPath && (
          <>
            <dt className="text-fg-muted">Data folder</dt>
            <dd className="flex items-center gap-1 font-mono break-all">
              {s.dataPath}
              <CopyButton value={s.dataPath} label="data folder path" />
            </dd>
          </>
        )}
      </dl>
      {isDb && (
        <section aria-labelledby="conn-heading" className="rounded-md border border-border p-4">
          <h2 id="conn-heading" className="font-semibold">
            Connection
          </h2>
          <p className="mt-1 font-mono">host 127.0.0.1 · port {s.port}</p>
          <p className="mt-1 text-fg-muted">
            User, connection URL, credential notes and backups are on the{" "}
            <Link to="/databases" className="text-accent hover:underline">
              Databases
            </Link>{" "}
            page.
          </p>
        </section>
      )}
      {s.role !== "fast_cgi" && <PortsForm service={s} disabled={live} />}
      <div>
        <Button onClick={() => setRemoving(true)} disabled={live} aria-label={`Remove ${s.instanceName}`}>
          Remove service
        </Button>
        {live && <p className="mt-1 text-xs text-fg-muted">Stop the service before removing it.</p>}
      </div>
      {removing && (
        <ConfirmChangeDialog
          open
          destructive
          onOpenChange={(open) => !open && setRemoving(false)}
          title={`Remove ${s.instanceName}?`}
          target={s.configPath ?? s.instanceName}
          impact={
            <>
              Devluma deletes this service&apos;s generated configuration and logs.
              {s.dataPath ? (
                <>
                  {" "}
                  The data folder <span className="font-mono break-all">{s.dataPath}</span> is kept.
                </>
              ) : null}
            </>
          }
          confirmLabel="Remove service"
          onConfirm={async () => {
            await ipc.removeService(s.id);
            navigate("/services");
          }}
        />
      )}
    </div>
  );
}

function Configuration({ serviceId }: { serviceId: string }) {
  const details = useAsync(() => ipc.getServiceDetails(serviceId));
  if (details.status === "pending")
    return <LoadingState label="Loading configuration…" shape="panel" rows={6} />;
  if (details.status === "error") return <ErrorPanel error={details.error} onRetry={details.reload} />;
  const d = details.data;
  return (
    <div className="flex flex-col gap-2 text-sm">
      <p className="flex items-center gap-1 font-mono text-xs break-all text-fg-muted">
        {d.configPath}
        <CopyButton value={d.configPath} label="configuration path" />
      </p>
      {d.configText === null ? (
        <p className="text-fg-muted">The configuration file hasn&apos;t been created yet.</p>
      ) : (
        <pre className="max-h-96 overflow-auto rounded-md bg-surface-sunken p-3 font-mono text-xs">
          {d.configText}
        </pre>
      )}
    </div>
  );
}

function Logs({ serviceId }: { serviceId: string }) {
  const details = useAsync(() => ipc.getServiceDetails(serviceId));
  const [file, setFile] = useState(0);
  if (details.status === "pending") return <LoadingState label="Loading logs…" shape="panel" rows={6} />;
  if (details.status === "error") return <ErrorPanel error={details.error} onRetry={details.reload} />;
  const logs = details.data.logs;
  const current = logs[file] ?? logs[0];
  return (
    <div className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Log file</span>
          <select
            value={file}
            onChange={(e) => setFile(Number(e.target.value))}
            className="rounded-md border border-border bg-surface px-2 py-1.5"
          >
            {logs.map((l, i) => (
              <option key={l.path} value={i}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
        <Button onClick={details.refresh}>Refresh</Button>
        {current && <CopyButton value={current.lines.join("\n")} label="log lines" />}
      </div>
      <p className="text-xs text-fg-muted">Last 200 lines. Secrets are redacted before display.</p>
      {current && current.lines.length > 0 ? (
        <pre className="max-h-96 overflow-auto rounded-md bg-surface-sunken p-3 font-mono text-xs">
          {current.lines.join("\n")}
        </pre>
      ) : (
        <p className="text-fg-muted">This log is empty.</p>
      )}
    </div>
  );
}
