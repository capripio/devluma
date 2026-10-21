import { useState, type FormEvent } from "react";
import { Link } from "react-router";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ServiceStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { ProjectView, ServiceView, StartAll } from "../../types/dto";
import { portConflictText } from "../services/portConflicts";

/** Services a user can choose for a project (PHP backends come with sites automatically). */
export function choosableServices(services: ServiceView[]): ServiceView[] {
  return services.filter((s) => s.role !== "fast_cgi");
}

/** Preselection for a new project: the first service of each kind (web server, database, mail…). */
export function defaultServiceIds(services: ServiceView[]): string[] {
  const seen = new Set<string>();
  return choosableServices(services)
    .filter((s) => !seen.has(s.serviceType) && seen.add(s.serviceType))
    .map((s) => s.id);
}

/** Checkboxes for the services a project uses, with a warning for taken ports. */
export function ServiceChecklist({
  services,
  selected,
  onChange,
  legend,
}: {
  services: ServiceView[];
  selected: string[];
  onChange: (ids: string[]) => void;
  legend: string;
}) {
  const choices = choosableServices(services);
  return (
    <fieldset className="flex flex-col gap-1">
      <legend className="mb-1 text-fg-muted">{legend}</legend>
      {choices.length === 0 && (
        <p className="text-xs text-fg-muted">
          No services yet. Create them on the{" "}
          <Link to="/services" className="text-accent hover:underline">
            Services
          </Link>{" "}
          page, then choose them here.
        </p>
      )}
      {choices.map((s) => {
        const conflict = portConflictText(s);
        return (
          <div key={s.id} className="flex flex-col">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={selected.includes(s.id)}
                onChange={(e) =>
                  onChange(e.target.checked ? [...selected, s.id] : selected.filter((id) => id !== s.id))
                }
              />
              <span>
                {s.instanceName}{" "}
                <span className="text-fg-muted">
                  ({s.packageName}
                  {s.port !== null && `, port ${s.port}`})
                </span>
              </span>
            </label>
            {conflict && <span className="ml-6 text-xs text-warning">{conflict}</span>}
          </div>
        );
      })}
    </fieldset>
  );
}

/** Edit the services an existing project uses. */
export function ChooseServicesForm({
  project,
  services,
  onDone,
}: {
  project: ProjectView;
  services: ServiceView[];
  onDone: (saved: boolean) => void;
}) {
  const [selected, setSelected] = useState(project.serviceIds);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await ipc.setProjectServices(project.id, selected);
      onDone(true);
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setPending(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      aria-label={`Services for ${project.name}`}
      className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm"
    >
      <ServiceChecklist
        services={services}
        selected={selected}
        onChange={setSelected}
        legend={`Services ${project.name} uses — "Start all" starts these, plus what its sites need.`}
      />
      {error && <ErrorPanel error={error} />}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" pending={pending}>
          Save
        </Button>
        <Button onClick={() => onDone(false)} disabled={pending}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

/** The services "Start all" covers for a project, each with its state. */
export function ProjectServiceList({ project, services }: { project: ProjectView; services: ServiceView[] }) {
  const mine = project.startServiceIds.flatMap((id) => services.filter((s) => s.id === id));
  if (mine.length === 0) return <span className="text-fg-muted">None chosen</span>;
  return (
    <ul className="flex flex-col gap-1">
      {mine.map((s) => {
        const conflict = portConflictText(s);
        return (
          <li key={s.id} className="flex flex-col">
            <span className="flex items-center gap-2">
              <ServiceStatusBadge state={s.observedState} />
              <span>{s.instanceName}</span>
            </span>
            {conflict && <span className="text-xs text-warning">{conflict}</span>}
          </li>
        );
      })}
    </ul>
  );
}

/** One line per outcome of "Start all", for the note under the project. */
export function startAllSummary(r: StartAll): { message: string; problems: string[] } {
  const parts: string[] = [];
  if (r.started.length > 0) parts.push(`Starting ${r.started.map((s) => s.serviceName).join(", ")}.`);
  if (r.alreadyRunning.length > 0) parts.push(`Already running: ${r.alreadyRunning.join(", ")}.`);
  if (parts.length === 0) parts.push("Nothing was started.");
  return {
    message: parts.join(" "),
    problems: r.notStarted.map((n) => `${n.serviceName} wasn't started: ${n.message}`),
  };
}
