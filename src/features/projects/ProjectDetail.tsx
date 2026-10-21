import { ExternalLink, ListChecks, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router";
import { Commands } from "../../app/commandBar";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { MoreMenu } from "../../components/MoreMenu";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ProjectView, ServiceView, SiteView, SwitchRequest } from "../../types/dto";
import { RuntimeSwitchDialog } from "../runtimes/RuntimeSwitchDialog";
import { SiteForm } from "../sites/SiteForm";
import { terminalOpenedMessage } from "../terminal/terminal";
import { ChooseServicesForm, ProjectServiceList, startAllSummary } from "./ProjectServices";

/** Inline text action inside the fact list ("Change", "Choose", "Add site"). */
const linkButton = "text-sm text-accent hover:underline disabled:opacity-60";

/** One project: where it lives, its website, runtimes and services, and what you can do with it. */
export function ProjectDetail({
  project: p,
  sites,
  services,
  initialSiteDomain,
  onChanged,
}: {
  project: ProjectView;
  sites: SiteView[];
  services: ServiceView[];
  /** Set right after adding the project with "Also create a site". */
  initialSiteDomain?: string;
  onChanged: () => void;
}) {
  const navigate = useNavigate();
  const [panel, setPanel] = useState<"services" | "site" | null>(initialSiteDomain ? "site" : null);
  const [changing, setChanging] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  // Result of the last action, read out politely.
  const [note, setNote] = useState<{ message?: string; problems?: string[]; error?: AppError } | null>(null);

  const report = <T,>(pending: string, run: () => Promise<T>, done: (r: T) => typeof note) => {
    setNote({ message: pending });
    run().then(
      (r) => setNote(done(r)),
      (e: unknown) => setNote({ error: toAppError(e) }),
    );
  };
  const startAll = () => report("Starting…", () => ipc.startProjectServices(p.id), startAllSummary);
  const openTerminal = () =>
    report(
      "Opening…",
      () => ipc.openProjectTerminal(p.id),
      (r) => ({ message: terminalOpenedMessage(r) }),
    );
  const openSite = (s: SiteView) =>
    report(
      "Opening…",
      () => ipc.openSite(s.id),
      (url) => ({ message: `Opened ${url} in your browser.` }),
    );

  return (
    <>
      <PageHeader title={p.name} description={`${p.detection.label} project`} />
      <Commands>
        <Button
          variant="primary"
          onClick={startAll}
          disabled={p.startServiceIds.length === 0}
          aria-label={`Start all services for ${p.name}`}
        >
          Start all
        </Button>
        {sites.length === 1 && (
          <Button onClick={() => openSite(sites[0]!)}>
            <ExternalLink aria-hidden className="size-4" />
            Open website
          </Button>
        )}
        <Button onClick={openTerminal} aria-label={`Open terminal for ${p.name}`}>
          Open terminal
        </Button>
        <MoreMenu
          label={`More actions for ${p.name}`}
          items={[
            { label: "Choose services", icon: ListChecks, onSelect: () => setPanel("services") },
            { label: "Add a site", icon: Plus, onSelect: () => setPanel("site") },
            {
              label: "Remove from Devluma",
              icon: Trash2,
              destructive: true,
              onSelect: () => setRemoving(true),
            },
          ]}
        />
      </Commands>

      {note && (
        <div aria-live="polite" className="flex flex-col gap-1 text-sm">
          {note.error ? (
            <p className="text-danger">{note.error.message}</p>
          ) : (
            <p className="text-fg-muted">{note.message}</p>
          )}
          {note.problems?.map((m) => (
            <p key={m} className="text-warning">
              {m}
            </p>
          ))}
        </div>
      )}

      <dl className="grid max-w-3xl grid-cols-[6rem_1fr] gap-x-4 gap-y-4 text-sm">
        <dt className="text-fg-muted">Folder</dt>
        <dd className="flex items-center gap-1 font-mono text-xs">
          <span className="break-all">{p.rootPath}</span>
          <CopyButton value={p.rootPath} label={`folder of ${p.name}`} />
        </dd>

        <dt className="text-fg-muted">Website</dt>
        <dd className="flex flex-wrap items-center gap-x-4 gap-y-1">
          {sites.map((s) => (
            <button
              key={s.id}
              type="button"
              onClick={() => openSite(s)}
              aria-label={`Open ${s.domain}`}
              className="inline-flex items-center gap-1 font-mono text-accent hover:underline"
            >
              {s.httpsUrl ?? s.url}
              <ExternalLink aria-hidden className="size-3.5" />
            </button>
          ))}
          {sites.length === 0 && <span className="text-fg-muted">No site yet</span>}
          <button type="button" className={linkButton} onClick={() => setPanel("site")}>
            Add a site
          </button>
        </dd>

        {p.runtimes.map((rt) => (
          <RuntimeRow
            key={rt.packageId}
            project={p}
            runtime={rt}
            onChange={() => setChanging(rt.packageId)}
          />
        ))}

        <dt className="text-fg-muted">Services</dt>
        <dd className="flex flex-col items-start gap-2">
          <ProjectServiceList project={p} services={services} />
          <button
            type="button"
            className={linkButton}
            onClick={() => setPanel("services")}
            aria-label={`Choose services for ${p.name}`}
          >
            Choose services
          </button>
        </dd>
      </dl>

      {p.detection.kind === "python" && <VenvHint project={p} />}

      {panel === "services" && (
        <ChooseServicesForm
          project={p}
          services={services}
          onDone={(saved) => {
            setPanel(null);
            if (saved) onChanged();
          }}
        />
      )}
      {panel === "site" && (
        <section aria-label={`Site for ${p.name}`} className="flex flex-col gap-2 text-sm">
          {initialSiteDomain && (
            <p className="text-fg-muted">
              {p.name} was added. Review its site; nothing changes until you apply.
            </p>
          )}
          <SiteForm initialProjectId={p.id} initialDomain={initialSiteDomain} onDone={() => setPanel(null)} />
        </section>
      )}

      {changing && (
        <ProjectRuntimeDialog
          project={p}
          packageId={changing}
          onClose={() => setChanging(null)}
          onApplied={onChanged}
        />
      )}
      <ConfirmChangeDialog
        open={removing}
        destructive
        onOpenChange={setRemoving}
        title={`Remove ${p.name} from Devluma?`}
        target={p.rootPath}
        impact="Devluma forgets this project and its runtime choices. The folder and every file in it stay exactly where they are."
        confirmLabel="Remove from Devluma"
        onConfirm={async () => {
          await ipc.removeProject(p.id);
          onChanged();
          navigate("/projects");
        }}
      />
    </>
  );
}

function RuntimeRow({
  project,
  runtime: rt,
  onChange,
}: {
  project: ProjectView;
  runtime: ProjectView["runtimes"][number];
  onChange: () => void;
}) {
  return (
    <>
      <dt className="text-fg-muted">{rt.packageName}</dt>
      <dd className="flex flex-wrap items-center gap-x-3">
        {rt.effective ? (
          <span>
            <span className="font-mono">{rt.effective.version}</span>{" "}
            <span className="text-fg-muted">
              ({rt.effective.source === "project_pin" ? "pinned for this project" : "global default"})
            </span>
          </span>
        ) : (
          <span className="text-fg-muted">Not set</span>
        )}
        <button
          type="button"
          className={linkButton}
          onClick={onChange}
          aria-label={`Change ${rt.packageName} for ${project.name}`}
        >
          Change
        </button>
      </dd>
    </>
  );
}

const USE_DEFAULT = "";

function ProjectRuntimeDialog({
  project,
  packageId,
  onClose,
  onApplied,
}: {
  project: ProjectView;
  packageId: string;
  onClose: () => void;
  onApplied: () => void;
}) {
  const packages = useAsync(ipc.listPackages);
  const runtime = project.runtimes.find((r) => r.packageId === packageId);
  const currentPin =
    runtime?.effective?.source === "project_pin" ? runtime.effective.installationId : USE_DEFAULT;
  const [choice, setChoice] = useState(currentPin);

  const pkg =
    packages.status === "success" ? packages.data.packages.find((p) => p.id === packageId) : undefined;
  const installed = (pkg?.versions ?? []).filter((v) => v.installation?.status === "installed");
  const name = runtime?.packageName ?? packageId;

  const request: SwitchRequest = {
    packageId,
    scope: "project",
    projectId: project.id,
    ...(choice === USE_DEFAULT ? {} : { installationId: choice }),
  };

  const selector = (
    <label className="flex flex-col gap-1">
      <span className="text-fg-muted">{name} version for this project</span>
      <select
        value={choice}
        onChange={(e) => setChoice(e.target.value)}
        className="rounded-md border border-border bg-surface px-2 py-1.5"
      >
        <option value={USE_DEFAULT}>Use the default</option>
        {installed.map((v) => (
          <option key={v.key} value={v.installation!.id}>
            {name} {v.version}
          </option>
        ))}
      </select>
      {packages.status === "success" && installed.length === 0 && (
        <span className="text-xs text-fg-muted">No {name} versions are installed yet.</span>
      )}
    </label>
  );

  return (
    <RuntimeSwitchDialog
      title={`${name} for ${project.name}`}
      request={request}
      selector={selector}
      confirmLabel="Apply"
      onClose={onClose}
      onApplied={onApplied}
    />
  );
}

/** Python projects: say whether a virtual environment is used, or how to make one. */
function VenvHint({ project }: { project: ProjectView }) {
  if (project.detection.virtualEnvScripts) {
    return (
      <p className="text-xs text-fg-muted">
        Virtual environment found — the project terminal uses it first ({project.detection.virtualEnvScripts}
        ).
      </p>
    );
  }
  const command = "python -m venv .venv";
  return (
    <p className="flex flex-wrap items-center gap-1 text-xs text-fg-muted">
      No virtual environment yet. In the project terminal, run <code className="font-mono">{command}</code>
      <CopyButton value={command} label="venv command" /> — Devluma then puts it first on PATH so packages
      stay in the project.
    </p>
  );
}
