import { useEffect, useRef, useState, type FormEvent } from "react";
import { Link } from "react-router";
import { isActive, useOperations } from "../../app/operations";
import { useStartupStatus } from "../../app/startup";
import { Button } from "../../components/Button";
import { CancelOperationButton } from "../../components/CancelOperationButton";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { AppPlan, AppProjectRequest, NewSiteOptions, ServiceView, WebApp } from "../../types/dto";
import { LocalCaNotice } from "../certificates/LocalCaNotice";
import { useServices } from "../services/useServices";
import { slugify, suggestDomain } from "../sites/SiteForm";
import { defaultServiceIds, ServiceChecklist } from "./ProjectServices";

/** What the project starts from: an empty (or existing) folder, or an app. */
type Start = "folder" | WebApp;
const STARTS: { value: Start; label: string; hint: string }[] = [
  { value: "folder", label: "Empty folder", hint: "Or a folder you already have in www" },
  { value: "wordpress", label: "WordPress", hint: "Copies the release, creates its database" },
  { value: "laravel", label: "Laravel", hint: "composer create-project laravel/laravel" },
  { value: "symfony", label: "Symfony", hint: "composer create-project symfony/skeleton" },
];

const LIVE = new Set(["starting", "running", "degraded"]);

/**
 * New project in `www\<folder>`, optionally from an app (ADR 0020). Its site is created
 * right away: the form says what will be created, so there is no separate review step.
 */
export function AddProjectForm({
  onCancel,
  onAdded,
}: {
  onCancel: () => void;
  onAdded: (projectId: string, note: string | null) => void;
}) {
  const projectsDir = useStartupStatus()?.projectsDir ?? "www";
  // The domain suffix from Settings; ".test" if preferences can't be read.
  const suffix = useAsync(() =>
    ipc.getPreferences().then(
      (p) => p.preferences.domainSuffix,
      () => "test",
    ),
  );
  const [start, setStart] = useState<Start>("folder");
  const [name, setName] = useState("");
  // Folder and domain follow the name until the user edits them.
  const [folder, setFolder] = useState<string | null>(null);
  const [domain, setDomain] = useState<string | null>(null);
  const [createSite, setCreateSite] = useState(true);
  const [webId, setWebId] = useState<string | null>(null);
  const [https, setHttps] = useState(false);
  const [dbId, setDbId] = useState<string | null>(null);
  const services = useServices();
  // Preselected once the services load: the first of each kind; `null` until the user edits.
  const [serviceIds, setServiceIds] = useState<string[] | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [running, setRunning] = useState<{ operationId: string; projectId: string } | null>(null);

  const serviceList = services.status === "success" ? services.data : [];
  const webServers = serviceList.filter((s) => s.role === "web_server");
  const mariadbs = serviceList.filter((s) => s.packageId === "mariadb");
  const serviceIdsValue = serviceIds ?? defaultServiceIds(serviceList);
  const folderValue = folder ?? slugify(name);
  const domainValue =
    domain ?? (name.trim() ? suggestDomain(name, suffix.status === "success" ? suffix.data : "test") : "");
  const webValue = webServers.find((w) => w.id === webId) ?? webServers[0];
  const siteWanted = createSite && webValue !== undefined;
  // WordPress: the first running MariaDB unless the user chose otherwise ("" = none).
  const dbValue = dbId ?? mariadbs.find((s) => LIVE.has(s.observedState))?.id ?? (mariadbs[0]?.id || "");

  const site: NewSiteOptions | undefined =
    siteWanted && webValue ? { domain: domainValue, webServerServiceId: webValue.id, https } : undefined;
  const appRequest: AppProjectRequest | null =
    start === "folder"
      ? null
      : {
          name,
          folder: folderValue,
          app: start,
          serviceIds: serviceIdsValue,
          ...(site ? { site } : {}),
          ...(start === "wordpress" && dbValue ? { databaseServiceId: dbValue } : {}),
        };
  const plan = useAppPlan(name.trim() && folderValue ? appRequest : null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending || running) return;
    setPending(true);
    setError(null);
    try {
      if (appRequest) {
        setRunning(await ipc.createAppProject(appRequest));
      } else {
        const created = await ipc.createProject(name, folderValue, serviceIdsValue, site ?? null);
        onAdded(
          created.project.id,
          created.siteError
            ? `${created.project.name} was added, but its site wasn't created: ${created.siteError}`
            : created.siteOperationId
              ? `Creating ${domainValue}…`
              : null,
        );
      }
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
      aria-label="Add project"
      className="flex flex-col gap-4 rounded-md border border-border bg-surface-raised p-4 text-sm"
    >
      <fieldset className="flex flex-col gap-2" disabled={locked}>
        <legend className="mb-1 text-fg-muted">Start from</legend>
        <div className="flex flex-wrap gap-2">
          {STARTS.map((s) => (
            <label
              key={s.value}
              className={`flex w-52 cursor-pointer flex-col gap-0.5 rounded-md border px-3 py-2 ${
                start === s.value ? "border-accent bg-surface" : "border-border hover:bg-surface"
              }`}
            >
              <span className="flex items-center gap-2 font-medium">
                <input
                  type="radio"
                  name="project-start"
                  value={s.value}
                  checked={start === s.value}
                  onChange={() => setStart(s.value)}
                />
                {s.label}
              </span>
              <span className="text-xs text-fg-muted">{s.hint}</span>
            </label>
          ))}
        </div>
      </fieldset>

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
              aria-describedby="project-folder-path"
              className={`${input} font-mono`}
            />
          </label>
          <span id="project-folder-path" className="font-mono text-xs break-all text-fg-muted">
            {projectsDir}\{folderValue || "…"} —{" "}
            {start === "folder" ? "created if it doesn't exist" : "must be new or empty"}
          </span>
        </div>
      </fieldset>

      <fieldset className="flex flex-col gap-2" disabled={locked}>
        <legend className="sr-only">Website</legend>
        {webServers.length === 0 ? (
          <p className="text-fg-muted">
            No web server yet, so no site is created. Add Nginx, Apache or Caddy on the{" "}
            <Link to="/services" className="text-accent hover:underline">
              Services
            </Link>{" "}
            page, then add a site from the project.
          </p>
        ) : (
          <>
            <div className="flex flex-wrap items-end gap-4">
              <label className="flex items-center gap-2 py-1.5">
                <input
                  type="checkbox"
                  checked={createSite}
                  onChange={(e) => setCreateSite(e.target.checked)}
                />
                Create a website at
              </label>
              <label className="flex flex-col gap-1">
                <span className="sr-only">Domain</span>
                <input
                  required={createSite}
                  disabled={!createSite}
                  value={domainValue}
                  onChange={(e) => setDomain(e.target.value)}
                  placeholder="my-app.test"
                  className={`${input} w-56 font-mono disabled:opacity-60`}
                />
              </label>
              {webServers.length > 1 ? (
                <label className="flex flex-col gap-1">
                  <span className="text-fg-muted">Web server</span>
                  <select
                    value={webValue?.id}
                    disabled={!createSite}
                    onChange={(e) => setWebId(e.target.value)}
                    className={input}
                  >
                    {webServers.map((w) => (
                      <option key={w.id} value={w.id}>
                        {w.instanceName} — {w.packageName} (port {w.port})
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <span className="py-1.5 text-fg-muted">
                  on {webValue?.instanceName} (port {webValue?.port})
                </span>
              )}
            </div>
            {createSite && (
              <>
                <label className="flex items-center gap-2">
                  <input type="checkbox" checked={https} onChange={(e) => setHttps(e.target.checked)} />
                  Also serve over HTTPS with a local development certificate
                </label>
                {https && <LocalCaNotice domain={domainValue} />}
                <p className="text-xs text-fg-muted">
                  The site gets its web server config and a line in the Windows hosts file (Windows may ask
                  for permission). Review or change it later on the project page.
                </p>
              </>
            )}
          </>
        )}
      </fieldset>

      {start === "wordpress" && (
        <WordpressDatabase services={mariadbs} value={dbValue} onChange={setDbId} disabled={locked} />
      )}

      {services.status === "success" && (
        <fieldset disabled={locked}>
          <ServiceChecklist
            services={services.data}
            selected={serviceIdsValue}
            onChange={setServiceIds}
            legend={'Services this project uses ("Start all" starts them)'}
          />
        </fieldset>
      )}

      {appRequest && <PlanSummary plan={plan} />}
      {error && <ErrorPanel error={error} />}
      {running && (
        <AppProgress
          operationId={running.operationId}
          onDone={() => onAdded(running.projectId, null)}
          onFailed={() => setRunning(null)}
        />
      )}

      {!running && (
        <div className="flex gap-2">
          <Button
            type="submit"
            variant="primary"
            pending={pending}
            disabled={Boolean(appRequest && plan?.blocked)}
          >
            {appRequest ? `Create ${STARTS.find((s) => s.value === start)?.label} project` : "Add project"}
          </Button>
          <Button onClick={onCancel} disabled={pending}>
            Cancel
          </Button>
        </div>
      )}
    </form>
  );
}

/** The app plan for the current request, refreshed shortly after the user stops typing. */
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
            {LIVE.has(s.observedState) ? "" : ", not running"})
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
          {op.status === "cancelled" ? "Cancelled. Nothing was added." : "The project wasn't created."}
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
        <span className="font-medium">Creating the project… </span>
        <span className="font-mono text-xs text-fg-muted">{op?.phase ?? "Starting"}</span>
      </p>
      <CancelOperationButton operationId={operationId} label="Cancel creating the project" />
    </div>
  );
}
