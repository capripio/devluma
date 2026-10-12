import { Server } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router";
import { useOperations } from "../../app/operations";
import { Button } from "../../components/Button";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ServiceStatusBadge } from "../../components/StatusBadge";
import type { ServiceView } from "../../types/dto";
import { AddServiceForm } from "./AddServiceForm";
import { ServiceActions } from "./ServiceActions";
import { useServices } from "./useServices";

// design.md §5 groups. Redis and Memcached are left out on Windows (ADR 0011).
const GROUPS: { role: ServiceView["role"]; title: string }[] = [
  { role: "web_server", title: "Web servers" },
  { role: "database", title: "Databases" },
  { role: "mail", title: "Mail" },
  { role: "fast_cgi", title: "PHP (FastCGI)" },
];

const HEALTH_LABELS: Record<string, string> = {
  healthy: "Accepting connections",
  unreachable: "Not accepting connections",
  unknown: "—",
};

export function ServicesPage() {
  const services = useServices();
  const [adding, setAdding] = useState(false);

  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={adding}>
      Add service
    </Button>
  );
  return (
    <>
      <PageHeader
        title="Services"
        description="Web servers and databases managed by Devluma. They listen on 127.0.0.1 only."
        actions={addButton}
      />
      {adding && <AddServiceForm onDone={() => setAdding(false)} />}
      {services.status === "pending" && (
        <p role="status" className="text-sm text-fg-muted">
          Loading services…
        </p>
      )}
      {services.status === "error" && <ErrorPanel error={services.error} onRetry={services.reload} />}
      {services.status === "success" &&
        (services.data.length === 0
          ? !adding && (
              <EmptyState
                icon={Server}
                title="No services yet"
                description="Install a web server, database or Mailpit from Runtimes & Packages, then add a service to run it."
                action={addButton}
              />
            )
          : GROUPS.map((g) => {
              const items = services.data.filter((s) => s.role === g.role);
              return items.length === 0 ? null : (
                <ServiceGroup key={g.title} title={g.title} services={items} />
              );
            }))}
    </>
  );
}

function ServiceGroup({ title, services }: { title: string; services: ServiceView[] }) {
  const { operations } = useOperations();
  const lastFailure = (s: ServiceView) => {
    const op = operations.find((o) => o.targetId === `service:${s.id}`);
    return op?.status === "failed" ? op.safeMessage : undefined;
  };
  const headingId = `group-${title.replace(/\W+/g, "-").toLowerCase()}`;
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-2">
      <h2 id={headingId} className="text-sm font-semibold">
        {title}
      </h2>
      <div className="overflow-x-auto rounded-lg border border-border">
        <table className="w-full text-left text-sm">
          <thead className="bg-surface-raised text-fg-muted">
            <tr>
              {["Service", "Version", "Status", "Port", "Health", "Actions"].map((h) => (
                <th key={h} scope="col" className="px-3 py-2 font-medium">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {services.map((s) => {
              const failure = lastFailure(s);
              return (
                <tr key={s.id} className="align-top">
                  <td className="px-3 py-2">
                    <Link to={`/services/${s.id}`} className="font-medium text-accent hover:underline">
                      {s.instanceName}
                    </Link>
                    {failure && s.observedState !== "running" && (
                      <p className="mt-1 max-w-md text-xs text-danger">{failure}</p>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    {s.packageName} {s.version}
                  </td>
                  <td className="px-3 py-2">
                    <ServiceStatusBadge state={s.observedState} />
                  </td>
                  <td className="px-3 py-2 font-mono">
                    {s.port ?? "—"}
                    {s.adminPort !== null && <div className="text-xs text-fg-muted">UI {s.adminPort}</div>}
                  </td>
                  <td className="px-3 py-2 text-fg-muted">
                    {HEALTH_LABELS[s.healthStatus] ?? s.healthStatus}
                  </td>
                  <td className="px-3 py-2">
                    <ServiceActions service={s} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
