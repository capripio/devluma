import { useEffect, useRef, useState, type FormEvent } from "react";
import { isActive, useOperations } from "../../app/operations";
import { useStartupStatus } from "../../app/startup";
import { Button } from "../../components/Button";
import { CancelOperationButton } from "../../components/CancelOperationButton";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type {
  AppPlan,
  AppProjectRequest,
  AppVersions,
  ProjectView,
  ServiceView,
  WebApp,
} from "../../types/dto";
import { useServices } from "../services/useServices";
import { slugify } from "../sites/SiteForm";
import { useNewSite } from "./NewSite";
import { defaultServiceIds, ServiceChecklist } from "./ProjectServices";

const APPS: { value: WebApp; label: string; hint: string }[] = [
  { value: "wordpress", label: "WordPress", hint: "Downloaded into the project, with its own database" },
  { value: "laravel", label: "Laravel", hint: "composer create-project laravel/laravel" },
  { value: "symfony", label: "Symfony", hint: "composer create-project symfony/skeleton" },
];

const LIVE = new Set(["starting", "running", "degraded"]);

/**
 * The web installer (ADR 0020): WordPress, Laravel or Symfony, installed into a project only:
 * a new project in `www`, or `project` when its folder is empty. Nothing is installed globally:
 * WordPress is downloaded straight into the project; Composer is the one shared tool.
 */
export function WebAppInstaller({
  project,
  hasSite = false,
  onCancel,
  onDone,
}: {
  /** Install into this existing project (its folder is empty) instead of a new one. */
  project?: ProjectView;
  /** The existing project has a site already, so none is offered. */
  hasSite?: boolean;
  onCancel: () => void;
  onDone: (projectId: string) => void;
}) {
  const projectsDir = useStartupStatus()?.projectsDir ?? "www";
  const [app, setApp] = useState<WebApp>("wordpress");
  const [version, setVersion] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [folder, setFolder] = useState<string | null>(null);
  const [dbId, setDbId] = useState<string | null>(null);
  const services = useServices();
  const [serviceIds, setServiceIds] = useState<string[] | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [running, setRunning] = useState<{ operationId: string; projectId: string } | null>(null);
  const versions = useAppVersions(app, project?.id ?? null);

  const serviceList = services.status === "success" ? services.data : [];
  const mariadbs = serviceList.filter((s) => s.packageId === "mariadb");
  const serviceIdsValue = serviceIds ?? defaultServiceIds(serviceList);
  const folderValue = folder ?? slugify(name);
  const { site, fields: siteFields } = useNewSite(project?.name ?? name, serviceList);
  // WordPress: the first running MariaDB unless the user chose otherwise ("" = none).
  const dbValue = dbId ?? mariadbs.find((s) => LIVE.has(s.observedState))?.id ?? (mariadbs[0]?.id || "");
  // The newest release this PHP can run, unless the user picked one.
  const versionValue =
    version ?? versions?.versions.find((v) => v.supported)?.value ?? versions?.versions[0]?.value;

  const request: AppProjectRequest = {
    name: project?.name ?? name,
    folder: project ? "" : folderValue,
    app,
    serviceIds: project ? [] : serviceIdsValue,
    ...(site && !hasSite ? { site } : {}),
    ...(app === "wordpress" && dbValue ? { databaseServiceId: dbValue } : {}),
    ...(versionValue ? { version: versionValue } : {}),
    ...(project ? { projectId: project.id } : {}),
  };
  const ready = project !== undefined || (name.trim() !== "" && folderValue !== "");
  const plan = useAppPlan(ready && versions ? request : null);
  const label = APPS.find((a) => a.value === app)!.label;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending || running) return;
    setPending(true);
    setError(null);
    try {
      setRunning(await ipc.createAppProject(request));
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setPending(false);
    }
  };

  const input = "rounded-md border border-border bg-surface px-2 py-1.5";
  const locked = running !== null;
  return (
    <form
      onSubmit={submit}
      aria-label="Install a web app"
      className="flex flex-col gap-4 rounded-md border border-border bg-surface-raised p-4 text-sm"
    >
      <fieldset className="flex flex-col gap-2" disabled={locked}>
        <legend className="mb-1 text-fg-muted">
          {project ? `Install into ${project.name}` : "Web app for a new project"}
        </legend>
        <div className="flex flex-wrap gap-2">
          {APPS.map((a) => (
            <div
              key={a.value}
              className={`flex w-60 flex-col gap-0.5 rounded-md border px-3 py-2 ${
                app === a.value ? "border-accent bg-surface" : "border-border hover:bg-surface"
              }`}
            >
              <label className="flex cursor-pointer items-center gap-2 font-medium">
                <input
                  type="radio"
                  name="web-app"
                  value={a.value}
                  aria-describedby={`web-app-${a.value}`}
                  checked={app === a.value}
                  onChange={() => {
                    setApp(a.value);
                    setVersion(null);
                  }}
                />
                {a.label}
              </label>
              <span id={`web-app-${a.value}`} className="ml-6 text-xs text-fg-muted">
                {a.hint}
              </span>
            </div>
          ))}
        </div>
      </fieldset>

      <VersionChoice versions={versions} value={versionValue} onChange={setVersion} disabled={locked} />

      {project ? (
        <p className="text-fg-muted">
          Goes into <span className="font-mono text-xs">{project.rootPath}</span>, which is empty.
        </p>
      ) : (
        <fieldset className="flex flex-wrap gap-4" disabled={locked}>
          <label className="flex flex-col gap-1">
            <span className="text-fg-muted">Name</span>
            <input
              required
              value={name}
              onChange={(e) => setName(e.target.value)}
              className={`${input} w-56`}
            />
          </label>
          <div className="flex min-w-72 flex-1 flex-col gap-1">
            <label className="flex flex-col gap-1">
              <span className="text-fg-muted">Folder</span>
              <input
                required
                value={folderValue}
                onChange={(e) => setFolder(e.target.value)}
                placeholder="my-app"
                aria-describedby="web-app-folder-path"
                className={`${input} font-mono`}
              />
            </label>
            <span id="web-app-folder-path" className="font-mono text-xs break-all text-fg-muted">
              {projectsDir}\{folderValue || "…"} — must be new or empty
            </span>
          </div>
        </fieldset>
      )}

      {!hasSite && siteFields(locked)}

      {app === "wordpress" && (
        <WordpressDatabase services={mariadbs} value={dbValue} onChange={setDbId} disabled={locked} />
      )}

      {!project && services.status === "success" && (
        <fieldset disabled={locked}>
          <ServiceChecklist
            services={services.data}
            selected={serviceIdsValue}
            onChange={setServiceIds}
            legend={'Services this project uses ("Start all" starts them)'}
          />
        </fieldset>
      )}

      {ready && <PlanSummary plan={plan} />}
      {error && <ErrorPanel error={error} />}
      {running && (
        <AppProgress
          operationId={running.operationId}
          onDone={() => onDone(running.projectId)}
          onFailed={() => setRunning(null)}
        />
      )}
      {!running && (
        <div className="flex gap-2">
          <Button type="submit" variant="primary" pending={pending} disabled={!plan || Boolean(plan.blocked)}>
            Install {label}
          </Button>
          <Button onClick={onCancel} disabled={pending}>
            Cancel
          </Button>
        </div>
      )}
    </form>
  );
}

/** Releases of `app` (judged against the project's PHP, or the default). */
function useAppVersions(app: WebApp, projectId: string | null): AppVersions | null {
  const [state, setState] = useState<{ app: WebApp; versions: AppVersions } | null>(null);
  useEffect(() => {
    let stale = false;
    ipc.listAppVersions(app, projectId).then(
      (v) => !stale && setState({ app, versions: v }),
      () =>
        !stale &&
        setState({
          app,
          versions: { app, versions: [], source: "", unavailable: "Couldn't load the releases." },
        }),
    );
    return () => {
      stale = true;
    };
  }, [app, projectId]);
  return state && state.app === app ? state.versions : null;
}

function VersionChoice({
  versions,
  value,
  onChange,
  disabled,
}: {
  versions: AppVersions | null;
  value: string | undefined;
  onChange: (v: string) => void;
  disabled: boolean;
}) {
  if (!versions) return <p className="text-fg-muted">Loading the releases…</p>;
  if (versions.versions.length === 0) {
    return <p className="text-fg-muted">{versions.unavailable ?? "No releases listed."}</p>;
  }
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor="web-app-version" className="text-fg-muted">
        Version
      </label>
      <select
        id="web-app-version"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        aria-describedby="web-app-version-source"
        className="w-96 rounded-md border border-border bg-surface px-2 py-1.5"
      >
        {versions.versions.map((v) => (
          <option key={v.value} value={v.value} disabled={!v.supported}>
            {v.label}
            {v.label.endsWith(v.latest) ? "" : ` (${v.latest})`}
            {v.supported ? "" : ` — needs PHP ${v.minPhp}`}
          </option>
        ))}
      </select>
      <span id="web-app-version-source" className="text-xs text-fg-muted">
        Releases from {versions.source}.
      </span>
    </div>
  );
}

/** The plan for the current request, refreshed shortly after the user stops typing. */
function useAppPlan(request: AppProjectRequest | null): AppPlan | null {
  const key = request ? JSON.stringify(request) : "";
  const [plan, setPlan] = useState<{ key: string; plan: AppPlan } | null>(null);
  useEffect(() => {
    if (!key) return;
    let stale = false;
    const timer = setTimeout(() => {
      ipc.previewAppProject(JSON.parse(key) as AppProjectRequest).then(
        (p) => !stale && setPlan({ key, plan: p }),
        () => {},
      );
    }, 300);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [key]);
  return plan && plan.key === key ? plan.plan : null;
}

function WordpressDatabase({
  services,
  value,
  onChange,
  disabled,
}: {
  services: ServiceView[];
  value: string;
  onChange: (id: string) => void;
  disabled: boolean;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-fg-muted">Database</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        className="w-96 rounded-md border border-border bg-surface px-2 py-1.5"
      >
        {services.map((s) => (
          <option key={s.id} value={s.id}>
            Create one on {s.instanceName} (port {s.port}
            {LIVE.has(s.observedState) ? "" : ", started for you"})
          </option>
        ))}
        <option value="">None: enter the details in WordPress&apos;s setup</option>
      </select>
      {services.length === 0 && (
        <span className="text-xs text-fg-muted">
          No MariaDB service yet. WordPress&apos;s own setup will ask for a database.
        </span>
      )}
    </label>
  );
}

function PlanSummary({ plan }: { plan: AppPlan | null }) {
  if (!plan) return <p className="text-fg-muted">Checking what&apos;s needed…</p>;
  if (plan.blocked) {
    return (
      <p role="alert" className="text-warning">
        {plan.blocked}
      </p>
    );
  }
  return (
    <section aria-label="What happens" className="flex flex-col gap-1 rounded-md bg-surface p-3">
      <p className="font-medium">
        {plan.label} with PHP {plan.phpVersion}, in{" "}
        <span className="font-mono text-xs">{plan.targetPath}</span>
      </p>
      <ol className="list-inside list-decimal text-fg-muted">
        {plan.steps.map((s) => (
          <li key={s}>{s}</li>
        ))}
      </ol>
    </section>
  );
}

function AppProgress({
  operationId,
  onDone,
  onFailed,
}: {
  operationId: string;
  onDone: () => void;
  onFailed: () => void;
}) {
  const { operations } = useOperations();
  const op = operations.find((o) => o.operationId === operationId);
  const status = op?.status;
  // Once, even though the parent passes a new callback on every render.
  const done = useRef(onDone);
  const fired = useRef(false);
  useEffect(() => {
    done.current = onDone;
  });
  useEffect(() => {
    if (status === "succeeded" && !fired.current) {
      fired.current = true;
      done.current();
    }
  }, [status]);
  if (op && !isActive(op) && op.status !== "succeeded") {
    return (
      <div role="alert" className="flex flex-col items-start gap-2 rounded-md border border-border p-3">
        <p className="font-medium text-danger">
          {op.status === "cancelled" ? "Cancelled. Nothing was added." : "The app wasn't installed."}
        </p>
        {op.safeMessage && (
          <pre className="max-h-48 overflow-auto text-xs whitespace-pre-wrap text-fg-muted">
            {op.safeMessage}
          </pre>
        )}
        <Button onClick={onFailed}>Back to the form</Button>
      </div>
    );
  }
  return (
    <div aria-live="polite" className="flex items-center justify-between gap-3 rounded-md bg-surface p-3">
      <p className="min-w-0 truncate">
        <span className="font-medium">Installing… </span>
        <span className="font-mono text-xs text-fg-muted">{op?.phase ?? "Starting"}</span>
      </p>
      <CancelOperationButton operationId={operationId} label="Cancel installing the app" />
    </div>
  );
}
