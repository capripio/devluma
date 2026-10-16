import { Server } from "lucide-react";
import { useState } from "react";
import { Link, NavLink, useParams } from "react-router";
import { useOperations } from "../../app/operations";
import { Button } from "../../components/Button";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ServiceStatusBadge } from "../../components/StatusBadge";
import type { ServiceView } from "../../types/dto";
import { AddServiceForm } from "./AddServiceForm";
import { ServiceActions } from "./ServiceActions";
import { ServiceDetail } from "./ServiceDetailPage";
import { HEALTH_LABELS } from "./ServiceInspector";
import { useServices } from "./useServices";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import { inkVar } from "../overview/StackMap";
import { layoutStackMap } from "../overview/stackMapLayout";
import { LoadingState } from "../../components/LoadingState";

// docs/design-brief.md §5 groups. Redis and Memcached are left out on Windows (ADR 0011).
const GROUPS: { role: ServiceView["role"]; title: string }[] = [
  { role: "web_server", title: "Web servers" },
  { role: "database", title: "Databases" },
  { role: "mail", title: "Mail" },
  { role: "fast_cgi", title: "PHP (FastCGI)" },
];

/**
 * Services: a grouped inventory, or, with a service selected, list and details side by side
 * (Windows list/details pattern). `/services/:serviceId` stays deep-linkable.
 */
export function ServicesPage() {
  const { serviceId } = useParams();
  return serviceId ? <ServicesSplit /> : <ServiceInventory />;
}

function ServicesSplit() {
  const services = useServices();
  const projects = useAsync(ipc.listProjects);
  const sites = useAsync(ipc.listSites);
  // The project lines through each service, from the same routes the Overview map draws.
  const layout =
    services.status === "success" && projects.status === "success" && sites.status === "success"
      ? layoutStackMap(projects.data, sites.data, services.data, 800)
      : null;
  const linesFor = (s: ServiceView): { id: string; name: string; ink: number }[] => {
    const station = layout?.stations.find((st) => st.serviceId === s.id);
    if (!layout || !station) return [];
    const seen = new Map<string, { id: string; name: string; ink: number }>();
    for (const r of layout.routes) {
      if (r.stationIds.includes(station.id) && !seen.has(r.projectId)) {
        seen.set(r.projectId, { id: r.projectId, name: r.projectName, ink: r.ink });
      }
    }
    return [...seen.values()];
  };
  return (
    <div className="flex min-h-0 flex-1">
      <nav
        aria-label="Services list"
        className="hidden w-72 shrink-0 flex-col gap-4 overflow-y-auto border-r border-border bg-surface-raised py-5 wide:flex"
      >
        <h2 className="label-caps px-5 text-fg-muted">Services</h2>
        {services.status === "success" &&
          GROUPS.map((g) => {
            const items = services.data.filter((s) => s.role === g.role);
            return items.length === 0 ? null : (
              <div key={g.title} className="flex flex-col gap-0.5 px-2">
                <span className="px-3 pb-1 text-xs font-semibold text-fg-muted">{g.title}</span>
                {items.map((s) => (
                  <NavLink
                    key={s.id}
                    to={`/services/${s.id}`}
                    className={({ isActive }) =>
                      `relative flex items-center justify-between gap-2 rounded-md px-3 py-2 text-sm transition-colors ${
                        isActive
                          ? "bg-surface font-semibold before:absolute before:top-2 before:bottom-2 before:left-0 before:w-[3px] before:rounded-full before:bg-marker"
                          : "hover:bg-surface-sunken"
                      }`
                    }
                  >
                    <span className="flex min-w-0 flex-col gap-0.5">
                      <span className="truncate">{s.instanceName}</span>
                      {/* The product name gives way first; the port is never hidden. */}
                      <span className="flex min-w-0 gap-1 text-xs font-normal whitespace-nowrap text-fg-muted">
                        <span className="truncate">
                          {s.packageName} <span className="font-mono">{s.version}</span>
                        </span>
                        {s.port !== null && <span className="shrink-0 font-mono">· :{s.port}</span>}
                      </span>
                      <LineChips lines={linesFor(s)} />
                    </span>
                    <ServiceStatusBadge state={s.observedState} />
                  </NavLink>
                ))}
              </div>
            );
          })}
      </nav>
      <div className="flex min-w-0 flex-1 flex-col gap-5 overflow-y-auto px-7 py-6">
        <ServiceDetail />
      </div>
    </div>
  );
}

function LineChips({ lines }: { lines: { id: string; name: string; ink: number }[] }) {
  if (lines.length === 0) return null;
  return (
    <span className="flex items-center gap-1">
      <span className="sr-only">Lines: {lines.map((l) => l.name).join(", ")}</span>
      {lines.map((l) => (
        <span
          key={l.id}
          aria-hidden
          title={l.name}
          className="h-1.5 w-4 rounded-full"
          style={{ background: inkVar(l.ink) }}
        />
      ))}
    </span>
  );
}

function ServiceInventory() {
  const services = useServices();
  const [adding, setAdding] = useState(false);

  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={adding}>
      Add service
    </Button>
  );
  return (
    <div className="flex flex-col gap-5 px-7 py-6">
      <PageHeader
        title="Services"
        description="Web servers and databases managed by Devluma. They listen on 127.0.0.1 only."
        actions={addButton}
      />
      {adding && <AddServiceForm onDone={() => setAdding(false)} />}
      {services.status === "pending" && <LoadingState label="Loading services…" shape="table" columns={6} />}
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
    </div>
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
      <h2 id={headingId} className="label-caps text-fg-muted">
        {title}
      </h2>
      <div className="overflow-x-auto rounded-md border border-border">
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
