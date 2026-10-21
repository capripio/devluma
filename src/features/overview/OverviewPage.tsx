import { ExternalLink, FolderPlus, Map as MapIcon, RefreshCw, Rows3, TriangleAlert } from "lucide-react";
import { useMemo, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router";
import {
  describeOperationType,
  needsRecovery,
  useOnOperationsSettled,
  useOperations,
} from "../../app/operations";
import { useStartupStatus } from "../../app/startup";
import { Commands } from "../../app/commandBar";
import { Button } from "../../components/Button";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
import { PageHeader } from "../../components/PageHeader";
import { OperationStatusBadge, ServiceStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import { useElementWidth } from "../../lib/useElementWidth";
import type { ProjectView, ServiceView, SiteView } from "../../types/dto";
import { terminalOpenedMessage } from "../terminal/terminal";
import { ServiceInspector } from "../services/ServiceInspector";
import { useServices } from "../services/useServices";
import { STATE_LABELS, StackMap, inkVar } from "./StackMap";
import { projectInks } from "./stackMapLayout";
import { isDelaying, layoutStackMap, type StackMapLayout, type Station } from "./stackMapLayout";

/** Operations that change projects, sites or runtime pins, so the map is redrawn after them. */
const MAP_OPERATIONS = new Set([
  "apply_site",
  "remove_site",
  "create_project",
  "remove_project",
  "switch_runtime",
]);

type View = "map" | "list";

// No fabricated metrics (docs/design-brief.md §5): everything here comes from the backend.
export function OverviewPage() {
  const services = useServices();
  const projects = useAsync(ipc.listProjects);
  const sites = useAsync(ipc.listSites);
  useOnOperationsSettled(
    (op) => MAP_OPERATIONS.has(op.operationType),
    () => {
      projects.refresh();
      sites.refresh();
    },
  );
  const [view, setView] = useState<View>("map");
  const [selected, setSelected] = useState<string | null>(null);
  const mapRef = useRef<HTMLDivElement>(null);
  const width = useElementWidth(mapRef, 760);

  const pending =
    services.status === "pending" || projects.status === "pending" || sites.status === "pending";
  const error =
    (services.status === "error" && services) ||
    (projects.status === "error" && projects) ||
    (sites.status === "error" && sites) ||
    null;

  const layout = useMemo(
    () =>
      services.status === "success" && projects.status === "success" && sites.status === "success"
        ? layoutStackMap(projects.data, sites.data, services.data, width - 34)
        : null,
    [services, projects, sites, width],
  );

  // Nothing chosen yet: point at whatever is holding a line up.
  const effectiveSelection =
    selected ?? layout?.stations.find((s) => s.serviceId && isDelaying(s.state))?.id ?? null;

  const refresh = () => {
    services.refresh();
    projects.refresh();
    sites.refresh();
  };

  return (
    // From 1000px: map and details side by side (pane widens at 1200px). Narrower: details follow.
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto wide:flex-row wide:overflow-hidden">
      <div className="flex min-w-0 flex-1 flex-col gap-5 px-7 py-6 wide:overflow-y-auto">
        <PageHeader
          title="Overview"
          description="Each coloured line is a project and the stack it runs on."
        />
        <Commands>
          <div role="group" aria-label="View" className="flex rounded border border-border-strong/70 p-0.5">
            {(
              [
                ["map", "Map", MapIcon],
                ["list", "List", Rows3],
              ] as const
            ).map(([id, label, Icon]) => (
              <button
                key={id}
                type="button"
                aria-pressed={view === id}
                onClick={() => setView(id)}
                className={`inline-flex h-7 items-center gap-1.5 rounded-sm px-2.5 text-sm transition-colors ${
                  view === id ? "bg-accent-subtle font-semibold text-fg" : "text-fg-muted hover:text-fg"
                }`}
              >
                <Icon aria-hidden className="size-4" />
                {label}
              </button>
            ))}
          </div>
          <Link
            to="/projects"
            className="inline-flex min-h-8 items-center gap-2 rounded border border-border-strong/70 bg-surface-raised px-3 text-sm font-medium transition-colors hover:bg-surface-sunken"
          >
            <FolderPlus aria-hidden className="size-4" />
            Add project
          </Link>
          <Button variant="ghost" onClick={refresh}>
            <RefreshCw aria-hidden className="size-4" />
            Refresh
          </Button>
        </Commands>
        <StartupNotice />

        <div ref={mapRef} className="min-w-0">
          {pending && <LoadingState label="Loading the stack map…" shape="panel" rows={5} />}
          {error && <ErrorPanel error={error.error} onRetry={error.reload} />}
          {layout &&
            (layout.routes.length === 0 ? (
              <EmptyState
                icon={MapIcon}
                title="No projects on the map yet"
                description="Add a project with a site and it appears here as a line."
                action={
                  <Link
                    to="/projects"
                    className="inline-flex rounded bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:bg-accent-hover"
                  >
                    Add a project
                  </Link>
                }
              />
            ) : view === "map" ? (
              <div className="overflow-x-auto rounded-md border border-border bg-map p-4">
                <StackMap layout={layout} selectedId={effectiveSelection} onSelect={setSelected} />
              </div>
            ) : (
              <RouteTable layout={layout} onSelect={setSelected} />
            ))}
        </div>

        {layout && layout.depot.length > 0 && (
          <Depot services={layout.depot} selectedId={effectiveSelection} onSelect={setSelected} />
        )}
      </div>

      <aside
        aria-label="Station details"
        className="shrink-0 border-t border-border bg-surface-raised px-5 py-6 wide:w-[18rem] wide:overflow-y-auto wide:border-t-0 wide:border-l split:w-[22rem]"
      >
        <DetailPane
          layout={layout}
          selectedId={effectiveSelection}
          services={services.status === "success" ? services.data : []}
          sites={sites.status === "success" ? sites.data : []}
          projects={projects.status === "success" ? projects.data : []}
        />
      </aside>
    </div>
  );
}

/** Services no project line runs through: nothing links a project to them. */
function Depot({
  services,
  selectedId,
  onSelect,
}: {
  services: ServiceView[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <section aria-labelledby="depot-heading" className="flex flex-col gap-2">
      <h2 id="depot-heading" className="label-caps text-fg-muted">
        Shared depot
      </h2>
      <p className="text-xs text-fg-muted">
        Databases, mail and other services. Projects aren&apos;t linked to them, so they sit off the lines.
      </p>
      <ul className="flex flex-wrap gap-2">
        {services.map((s) => {
          const id = `svc:${s.id}`;
          return (
            <li key={s.id}>
              <button
                type="button"
                aria-pressed={selectedId === id}
                onClick={() => onSelect(id)}
                className={`flex items-center gap-3 rounded-md border px-3 py-2 text-left text-sm transition-colors ${
                  selectedId === id
                    ? "border-focus bg-accent-subtle"
                    : "border-border bg-surface-raised hover:border-border-strong"
                }`}
              >
                <span className="flex flex-col">
                  <span className="font-semibold">
                    {s.packageName} <span className="font-mono text-xs font-normal">{s.version}</span>
                  </span>
                  <span className="font-mono text-xs text-fg-muted">
                    {s.port !== null ? `:${s.port}` : "—"}
                  </span>
                </span>
                <ServiceStatusBadge state={s.observedState} />
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** The map as a table: the accessible alternative and the better view for many projects. */
function RouteTable({ layout, onSelect }: { layout: StackMapLayout; onSelect: (id: string) => void }) {
  const station = (id: string | undefined) => layout.stations.find((s) => s.id === id);
  return (
    <div className="overflow-x-auto rounded-md border border-border">
      <table className="w-full text-left text-sm">
        <caption className="sr-only">Projects and the stack each site runs on</caption>
        <thead className="bg-surface-raised">
          <tr>
            {["Project", "PHP", "Web server", "Site", "Status"].map((h) => (
              <th key={h} scope="col" className="px-3 py-2 font-semibold">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {layout.routes.map((r) => {
            const stations = r.stationIds.map((id) => station(id));
            const php = stations.find((s) => s?.kind === "php");
            const web = stations.find((s) => s?.kind === "web");
            const site = stations.find((s) => s?.kind === "domain");
            const delay = station(r.delayedAt ?? undefined);
            return (
              <tr key={r.id}>
                <th scope="row" className="px-3 py-2 font-semibold">
                  <span className="flex items-center gap-2">
                    <span
                      aria-hidden
                      className="h-1.5 w-5 rounded-full"
                      style={{ background: inkVar(r.ink) }}
                    />
                    {r.projectName}
                  </span>
                </th>
                <StationCell station={php} onSelect={onSelect} />
                <StationCell station={web} onSelect={onSelect} />
                <StationCell station={site} onSelect={onSelect} />
                <td className="px-3 py-2">
                  {delay ? (
                    <span className="font-semibold text-warning">
                      Held up at {delay.label} ({STATE_LABELS[delay.state].toLowerCase()})
                    </span>
                  ) : (
                    <span className="text-success">Clear</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function StationCell({
  station,
  onSelect,
}: {
  station: Station | undefined;
  onSelect: (id: string) => void;
}) {
  if (!station) return <td className="px-3 py-2 text-fg-muted">—</td>;
  return (
    <td className="px-3 py-2">
      <button type="button" onClick={() => onSelect(station.id)} className="text-left hover:underline">
        {station.label}
        {station.detail && <span className="ml-1 font-mono text-xs text-fg-muted">{station.detail}</span>}
      </button>
    </td>
  );
}

function DetailPane({
  layout,
  selectedId,
  services,
  sites,
  projects,
}: {
  layout: StackMapLayout | null;
  selectedId: string | null;
  services: ServiceView[];
  sites: SiteView[];
  projects: ProjectView[];
}) {
  if (selectedId?.startsWith("proj:")) {
    const project = projects.find((p) => `proj:${p.id}` === selectedId);
    if (project) {
      return (
        <ProjectDetail
          project={project}
          ink={projectInks(projects).get(project.id) ?? 1}
          sites={sites.filter((s) => s.projectId === project.id)}
        />
      );
    }
  }
  const station = layout?.stations.find((s) => s.id === selectedId);
  const serviceId = selectedId?.startsWith("svc:") ? selectedId.slice(4) : (station?.serviceId ?? null);
  const service = services.find((s) => s.id === serviceId);
  const lines = station ? (layout?.routes.filter((r) => r.stationIds.includes(station.id)) ?? []) : [];

  if (!selectedId || (!station && !service)) return <Legend />;

  const linesServed = lines.length > 0 && (
    <section className="flex flex-col gap-2">
      <h3 className="label-caps text-fg-muted">Lines through here</h3>
      <ul className="flex flex-col gap-1.5 text-sm">
        {lines.map((r) => (
          <li key={r.id} className="flex items-center gap-2">
            <span aria-hidden className="h-1.5 w-5 rounded-full" style={{ background: inkVar(r.ink) }} />
            {r.projectName}
            {r.delayedAt && r.delayedAt !== station?.id && (
              <span className="text-xs text-warning">held up further along</span>
            )}
          </li>
        ))}
      </ul>
    </section>
  );

  if (service) {
    return (
      <ServiceInspector service={service}>
        {linesServed}
        <Link to={`/services/${service.id}`} className="text-sm font-medium text-accent hover:underline">
          Configuration and logs
        </Link>
      </ServiceInspector>
    );
  }
  if (station?.kind === "domain") {
    const site = sites.find((s) => s.id === station.siteId);
    return site ? (
      <SiteDetail site={site}>{linesServed}</SiteDetail>
    ) : (
      <ProjectWithoutSite>{linesServed}</ProjectWithoutSite>
    );
  }
  // PHP with no FastCGI service yet.
  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-1.5">
        <h2 className="font-display text-xl font-semibold">{station?.label}</h2>
        <span className="text-xs text-fg-muted">PHP runtime · no FastCGI service</span>
      </header>
      <p className="text-sm text-pretty">
        No FastCGI service runs for this PHP version yet. Sites that use it need one to serve PHP.
      </p>
      {linesServed}
      <Link to="/runtimes" className="text-sm font-medium text-accent hover:underline">
        Runtimes &amp; Packages
      </Link>
    </div>
  );
}

function SiteDetail({ site, children }: { site: SiteView; children: ReactNode }) {
  const [error, setError] = useState<AppError | null>(null);
  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-1.5">
        <h2 className="font-display text-xl font-semibold break-all">{site.domain}</h2>
        <span className="text-xs text-fg-muted">Site · {site.projectName}</span>
      </header>
      <dl className="grid grid-cols-[6rem_1fr] gap-x-3 gap-y-2 text-sm">
        <dt className="text-fg-muted">Address</dt>
        <dd className="font-mono text-xs break-all">{site.httpsUrl ?? site.url}</dd>
        <dt className="text-fg-muted">Web server</dt>
        <dd>{site.webServerName}</dd>
        <dt className="text-fg-muted">PHP</dt>
        <dd className="font-mono">
          {site.phpVersion ?? "None"}
          {site.phpOutdated && <span className="ml-2 font-sans text-xs text-warning">outdated</span>}
        </dd>
        <dt className="text-fg-muted">Root</dt>
        <dd className="font-mono text-xs break-all">{site.documentRoot}</dd>
      </dl>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          disabled={!site.webServerRunning}
          onClick={() => {
            setError(null);
            ipc.openSite(site.id).catch((e: unknown) => setError(toAppError(e)));
          }}
        >
          <ExternalLink aria-hidden className="size-4" />
          Open in browser
        </Button>
        <Link
          to="/sites"
          className="inline-flex min-h-8 items-center rounded border border-border-strong/70 bg-surface-raised px-3 text-sm font-medium hover:bg-surface-sunken"
        >
          Site settings
        </Link>
      </div>
      {!site.webServerRunning && (
        <p className="text-xs text-fg-muted">Start {site.webServerName} to open this site.</p>
      )}
      {error && <span className="text-xs text-danger">{error.message}</span>}
      {children}
    </div>
  );
}

function ProjectWithoutSite({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-1.5">
        <h2 className="font-display text-xl font-semibold">No site yet</h2>
      </header>
      <p className="text-sm text-pretty">
        This project has no local domain. Add a site to give it a web server and a .test address.
      </p>
      <Link to="/sites" className="text-sm font-medium text-accent hover:underline">
        Sites &amp; Domains
      </Link>
      {children}
    </div>
  );
}

/** A project line: where it lives, the PHP it runs, its sites, and a terminal set up for it. */
function ProjectDetail({ project, ink, sites }: { project: ProjectView; ink: number; sites: SiteView[] }) {
  const [terminal, setTerminal] = useState<{ message?: string; error?: AppError } | null>(null);
  const php = project.runtimes.find((r) => r.packageId === "php")?.effective ?? null;
  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-1.5">
        <h2 className="font-display text-xl font-semibold">{project.name}</h2>
        <span className="flex items-center gap-2 text-xs text-fg-muted">
          <span aria-hidden className="h-1.5 w-5 rounded-full" style={{ background: inkVar(ink) }} />
          Project · {project.detection.label}
        </span>
      </header>
      <dl className="grid grid-cols-[6rem_1fr] gap-x-3 gap-y-2 text-sm">
        <dt className="text-fg-muted">Folder</dt>
        <dd className="font-mono text-xs break-all select-all">{project.rootPath}</dd>
        <dt className="text-fg-muted">PHP</dt>
        <dd>
          {php ? (
            <>
              <span className="font-mono">{php.version}</span>{" "}
              <span className="text-xs text-fg-muted">
                {php.source === "project_pin" ? "pinned for this project" : "global default"}
              </span>
            </>
          ) : (
            <span className="text-fg-muted">None</span>
          )}
        </dd>
        <dt className="text-fg-muted">Sites</dt>
        <dd className="flex flex-col gap-0.5">
          {sites.length === 0 ? (
            <span className="text-fg-muted">None yet</span>
          ) : (
            sites.map((s) => (
              <span key={s.id} className="break-all">
                {s.domain}
              </span>
            ))
          )}
        </dd>
      </dl>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          onClick={() => {
            setTerminal({ message: "Opening…" });
            ipc.openProjectTerminal(project.id).then(
              (r) => setTerminal({ message: terminalOpenedMessage(r) }),
              (e: unknown) => setTerminal({ error: toAppError(e) }),
            );
          }}
        >
          Open terminal
        </Button>
        <Link
          to="/projects"
          className="inline-flex min-h-8 items-center rounded border border-border-strong/70 bg-surface-raised px-3 text-sm font-medium hover:bg-surface-sunken"
        >
          Project settings
        </Link>
      </div>
      {terminal?.message && <p className="text-xs text-fg-muted">{terminal.message}</p>}
      {terminal?.error && <p className="text-xs text-danger">{terminal.error.message}</p>}
    </div>
  );
}

/** Shown when nothing is selected: how to read the map, then recent activity. */
function Legend() {
  const { operations } = useOperations();
  const recent = operations.slice(0, 6);
  return (
    <div className="flex flex-col gap-6">
      <section className="flex flex-col gap-3">
        <h2 className="label-caps text-fg-muted">Reading the map</h2>
        <ul className="flex flex-col gap-3 text-sm">
          <li className="flex items-center gap-3">
            <svg aria-hidden width="36" height="10">
              <path d="M2 5H34" stroke="var(--dl-line-2)" strokeWidth="6" strokeLinecap="round" />
            </svg>
            A project&apos;s line
          </li>
          <li className="flex items-center gap-3">
            <span
              aria-hidden
              className="ml-2 h-6 w-5 rounded-full border-[3px] border-surface-raised bg-station shadow-[0_0_0_1px_var(--dl-border-strong)]"
            />
            <span className="ml-2">Shared station (interchange)</span>
          </li>
          <li className="flex items-center gap-3">
            <svg aria-hidden width="36" height="10">
              <path
                d="M2 5H34"
                stroke="var(--dl-line-1)"
                strokeWidth="6"
                strokeLinecap="round"
                strokeDasharray="2 11"
              />
            </svg>
            Held up by a station that isn&apos;t running
          </li>
        </ul>
        <p className="text-xs text-fg-muted">Select a station to see its details and actions.</p>
      </section>
      <section aria-labelledby="recent-ops-heading" className="flex flex-col gap-2">
        <h2 id="recent-ops-heading" className="label-caps text-fg-muted">
          Recent operations
        </h2>
        {recent.length === 0 ? (
          <p className="text-sm text-fg-muted">No operations yet.</p>
        ) : (
          <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
            {recent.map((op) => (
              <li key={op.operationId} className="flex flex-col gap-1 px-3 py-2 text-sm">
                <span className="flex items-center justify-between gap-2">
                  {describeOperationType(op.operationType)}
                  <OperationStatusBadge status={op.status} />
                </span>
                <span className="text-xs text-fg-muted">
                  {needsRecovery(op) ? "Interrupted when Devluma closed" : (op.safeMessage ?? op.phase ?? "")}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

/** What startup reconciliation changed, so nothing is silently re-labelled. */
function StartupNotice() {
  const recovery = useStartupStatus()?.recovery;
  if (!recovery) return null;
  const lines = [
    recovery.operationsNeedingRecovery > 0 &&
      `${recovery.operationsNeedingRecovery} operation${recovery.operationsNeedingRecovery === 1 ? " was" : "s were"} interrupted when Devluma last closed. Review before retrying.`,
    recovery.installationsMarkedBroken > 0 &&
      `${recovery.installationsMarkedBroken} installed package${recovery.installationsMarkedBroken === 1 ? " is" : "s are"} incomplete or missing files and ${recovery.installationsMarkedBroken === 1 ? "was" : "were"} marked broken.`,
  ].filter(Boolean);
  if (lines.length === 0) return null;
  return (
    <section
      aria-label="Startup checks"
      className="flex gap-3 rounded-md border border-warning/40 bg-warning-subtle p-4 text-sm"
    >
      <TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning" />
      <ul className="flex flex-col gap-1 text-fg">
        {lines.map((l) => (
          <li key={String(l)}>{l}</li>
        ))}
      </ul>
    </section>
  );
}
