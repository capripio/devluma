import { ExternalLink, FolderGit2, Plus } from "lucide-react";
import { useState } from "react";
import { Link, NavLink, useLocation, useNavigate, useParams } from "react-router";
import { Commands } from "../../app/commandBar";
import { useOnOperationsSettled } from "../../app/operations";
import { Button } from "../../components/Button";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ProjectView, ServiceView, SiteView } from "../../types/dto";
import { inkVar } from "../overview/StackMap";
import { projectInks } from "../overview/stackMapLayout";
import { useServices } from "../services/useServices";
import { OpenTerminalButton } from "../terminal/OpenTerminalButton";
import { AddProjectForm } from "./AddProjectForm";
import { ProjectDetail } from "./ProjectDetail";

const SITE_OPERATIONS = new Set(["apply_site", "remove_site"]);
const LIVE = new Set(["starting", "running", "degraded"]);

/** Navigation state: open the Add form, or hand a new project straight on to its site. */
interface ProjectsState {
  adding?: boolean;
  /** The project was just created; the list may not include it yet. */
  justAdded?: boolean;
  siteDomain?: string;
}

/** Projects: the list on the left, the overview or one project on the right. */
export function ProjectsPage() {
  const { projectId } = useParams();
  const location = useLocation();
  const navigate = useNavigate();
  const projects = useAsync(ipc.listProjects);
  const sites = useAsync(ipc.listSites);
  const services = useServices();
  // "Start all" also covers the web servers of a project's sites.
  useOnOperationsSettled((op) => SITE_OPERATIONS.has(op.operationType), projects.refresh);
  useOnOperationsSettled((op) => SITE_OPERATIONS.has(op.operationType), sites.refresh);

  const state = (location.state ?? {}) as ProjectsState;
  const list = projects.status === "success" ? projects.data : [];
  const siteList = sites.status === "success" ? sites.data : [];
  const serviceList = services.status === "success" ? services.data : [];
  const inks = projectInks(list);
  const sitesOf = (p: ProjectView) => siteList.filter((s) => s.projectId === p.id);
  const project = list.find((p) => p.id === projectId);

  let pane;
  if (projects.status === "pending")
    pane = <LoadingState label="Loading projects…" shape="table" columns={4} />;
  else if (projects.status === "error")
    pane = <ErrorPanel error={projects.error} onRetry={projects.reload} />;
  else if (projectId && !project)
    pane = state.justAdded ? (
      <LoadingState label="Loading the new project…" shape="panel" rows={3} />
    ) : (
      <PageHeader title="Project not found" description="It may have been removed." />
    );
  else if (project)
    pane = (
      <ProjectDetail
        key={project.id}
        project={project}
        sites={sitesOf(project)}
        services={serviceList}
        initialSiteDomain={state.siteDomain}
        onChanged={projects.refresh}
      />
    );
  else
    pane = (
      <ProjectsOverview
        projects={list}
        inks={inks}
        sitesOf={sitesOf}
        services={serviceList}
        adding={Boolean(state.adding)}
        setAdding={(adding) => navigate("/projects", { replace: true, state: { adding } })}
        onAdded={(id, domain) => {
          projects.refresh();
          navigate(`/projects/${id}`, {
            state: { justAdded: true, ...(domain ? { siteDomain: domain } : {}) },
          });
        }}
      />
    );

  return (
    <div className="flex min-h-0 flex-1">
      <nav
        aria-label="Project list"
        className="hidden w-72 shrink-0 flex-col gap-3 overflow-y-auto border-r border-border bg-surface-raised py-5 wide:flex"
      >
        <div className="flex items-center justify-between px-5">
          <Link to="/projects" className="label-caps text-fg-muted hover:text-fg">
            Projects
          </Link>
          <Link
            to="/projects"
            state={{ adding: true }}
            aria-label="Add project"
            className="rounded p-1 text-fg-muted hover:bg-surface-sunken hover:text-fg"
          >
            <Plus aria-hidden className="size-4" />
          </Link>
        </div>
        <div className="flex flex-col gap-0.5 px-2">
          {list.map((p) => (
            <NavLink
              key={p.id}
              to={`/projects/${p.id}`}
              className={({ isActive }) =>
                `relative flex items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors ${
                  isActive
                    ? "bg-surface font-semibold before:absolute before:top-2 before:bottom-2 before:left-0 before:w-[3px] before:rounded-full before:bg-marker"
                    : "hover:bg-surface-sunken"
                }`
              }
            >
              <InkSwatch ink={inks.get(p.id)} />
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="truncate">{p.name}</span>
                <span className="truncate font-mono text-xs font-normal text-fg-muted">
                  {sitesOf(p)[0]?.domain ?? "no site"}
                </span>
              </span>
            </NavLink>
          ))}
        </div>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col gap-5 overflow-y-auto px-7 py-6">{pane}</div>
    </div>
  );
}

/** The project's line ink, as on the Overview map (identity only, never status). */
function InkSwatch({ ink }: { ink: number | undefined }) {
  return (
    <span
      aria-hidden
      className="h-5 w-1.5 shrink-0 rounded-full"
      style={{ background: ink ? inkVar(ink) : "var(--dl-border-strong)" }}
    />
  );
}

/** "2 of 3 running" for the services Start all covers. */
function servicesSummary(p: ProjectView, services: ServiceView[]): string {
  const mine = services.filter((s) => p.startServiceIds.includes(s.id));
  if (mine.length === 0) return "None chosen";
  const running = mine.filter((s) => LIVE.has(s.observedState)).length;
  return running === mine.length ? "All running" : `${running} of ${mine.length} running`;
}

function ProjectsOverview({
  projects,
  inks,
  sitesOf,
  services,
  adding,
  setAdding,
  onAdded,
}: {
  projects: ProjectView[];
  inks: Map<string, number>;
  sitesOf: (p: ProjectView) => SiteView[];
  services: ServiceView[];
  adding: boolean;
  setAdding: (adding: boolean) => void;
  onAdded: (projectId: string, domain: string | null) => void;
}) {
  const [note, setNote] = useState<string | null>(null);
  const openSite = (s: SiteView) => {
    setNote("Opening…");
    ipc.openSite(s.id).then(
      (url) => setNote(`Opened ${url} in your browser.`),
      (e: unknown) => setNote(toAppError(e).message),
    );
  };
  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={adding}>
      Add project
    </Button>
  );

  return (
    <>
      <PageHeader title="Projects" description="Your projects in the www folder." />
      <Commands>
        {addButton}
        <OpenTerminalButton />
      </Commands>
      {adding && (
        <AddProjectForm onCancel={() => setAdding(false)} onAdded={(p, domain) => onAdded(p.id, domain)} />
      )}
      {note && (
        <p aria-live="polite" className="text-sm text-fg-muted">
          {note}
        </p>
      )}
      {projects.length === 0 ? (
        !adding && (
          <EmptyState
            icon={FolderGit2}
            title="No projects yet"
            description="Add one to get a folder in www, a local domain and its own PHP version."
            action={addButton}
          />
        )
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Projects</caption>
            <thead className="bg-surface-raised">
              <tr>
                {["Project", "Website", "PHP", "Services"].map((h) => (
                  <th key={h} scope="col" className="px-3 py-2">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {projects.map((p) => {
                const php = p.runtimes.find((r) => r.packageId === "php")?.effective;
                return (
                  <tr key={p.id}>
                    <th scope="row" className="px-3 py-2 font-medium">
                      <span className="flex items-center gap-3">
                        <InkSwatch ink={inks.get(p.id)} />
                        <Link to={`/projects/${p.id}`} className="text-accent hover:underline">
                          {p.name}
                        </Link>
                      </span>
                    </th>
                    <td className="px-3 py-2">
                      <span className="flex flex-col items-start gap-0.5">
                        {sitesOf(p).map((s) => (
                          <button
                            key={s.id}
                            type="button"
                            onClick={() => openSite(s)}
                            aria-label={`Open ${s.domain}`}
                            className="inline-flex items-center gap-1 font-mono text-xs text-accent hover:underline"
                          >
                            {s.domain}
                            <ExternalLink aria-hidden className="size-3" />
                          </button>
                        ))}
                        {sitesOf(p).length === 0 && <span className="text-fg-muted">—</span>}
                      </span>
                    </td>
                    <td className="px-3 py-2 font-mono text-xs">{php?.version ?? "—"}</td>
                    <td className="px-3 py-2 text-fg-muted">{servicesSummary(p, services)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
