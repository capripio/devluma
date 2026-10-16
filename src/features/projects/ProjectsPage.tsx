import { FolderGit2 } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ProjectView, SwitchRequest } from "../../types/dto";
import { RuntimeSwitchDialog } from "../runtimes/RuntimeSwitchDialog";
import { OpenTerminalButton } from "../terminal/OpenTerminalButton";
import { terminalOpenedMessage } from "../terminal/terminal";
import { LoadingState } from "../../components/LoadingState";

export function ProjectsPage() {
  const projects = useAsync(ipc.listProjects);
  const [adding, setAdding] = useState(false);

  const addButton = (
    <Button variant="primary" onClick={() => setAdding(true)} disabled={adding}>
      Add project
    </Button>
  );
  return (
    <>
      <PageHeader
        title="Projects"
        description="Local project folders and the runtime versions each one uses."
        actions={
          <>
            <OpenTerminalButton />
            {addButton}
          </>
        }
      />
      {adding && (
        <AddProjectForm
          onCancel={() => setAdding(false)}
          onAdded={() => {
            setAdding(false);
            projects.refresh();
          }}
        />
      )}
      {projects.status === "pending" && <LoadingState label="Loading projects…" shape="table" columns={5} />}
      {projects.status === "error" && <ErrorPanel error={projects.error} onRetry={projects.reload} />}
      {projects.status === "success" &&
        (projects.data.length === 0 ? (
          !adding && (
            <EmptyState
              icon={FolderGit2}
              title="No projects yet"
              description="Add a project folder to choose which PHP version it uses. Devluma never writes into your project folder, and removing a project never deletes it."
              action={addButton}
            />
          )
        ) : (
          <ProjectsTable projects={projects.data} onChanged={projects.refresh} />
        ))}
    </>
  );
}

function AddProjectForm({ onCancel, onAdded }: { onCancel: () => void; onAdded: () => void }) {
  const [name, setName] = useState("");
  const [rootPath, setRootPath] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await ipc.createProject(name, rootPath);
      onAdded();
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setPending(false);
    }
  };

  const input = "rounded-md border border-border bg-surface px-2 py-1.5";
  return (
    <form
      onSubmit={submit}
      aria-label="Add project"
      className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm"
    >
      <div className="flex flex-wrap gap-4">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Name</span>
          <input
            required
            value={name}
            onChange={(e) => setName(e.target.value)}
            className={`${input} w-56`}
          />
        </label>
        <label className="flex min-w-72 flex-1 flex-col gap-1">
          <span className="text-fg-muted">Folder</span>
          <input
            required
            value={rootPath}
            onChange={(e) => setRootPath(e.target.value)}
            placeholder="D:\code\my-app"
            className={`${input} font-mono`}
          />
        </label>
      </div>
      {error && <ErrorPanel error={error} />}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" pending={pending}>
          Add project
        </Button>
        <Button onClick={onCancel} disabled={pending}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function runtimeLabel(rt: ProjectView["runtimes"][number]): string {
  if (!rt.effective) return "Not set";
  return `${rt.effective.version} (${rt.effective.source === "project_pin" ? "pinned" : "default"})`;
}

function ProjectsTable({ projects, onChanged }: { projects: ProjectView[]; onChanged: () => void }) {
  const [changing, setChanging] = useState<{ project: ProjectView; packageId: string } | null>(null);
  const [removing, setRemoving] = useState<ProjectView | null>(null);
  const [terminal, setTerminal] = useState<{ projectId: string; message?: string; error?: AppError } | null>(
    null,
  );
  const runtimeColumns = projects[0]?.runtimes ?? [];

  const openTerminal = (p: ProjectView) => {
    setTerminal({ projectId: p.id, message: "Opening…" });
    ipc.openProjectTerminal(p.id).then(
      (r) => setTerminal({ projectId: p.id, message: terminalOpenedMessage(r) }),
      (e: unknown) => setTerminal({ projectId: p.id, error: toAppError(e) }),
    );
  };

  return (
    <>
      <div className="overflow-x-auto rounded-md border border-border">
        <table className="w-full text-left text-sm">
          <thead className="bg-surface-raised text-fg-muted">
            <tr>
              <th scope="col" className="px-3 py-2 font-medium">
                Project
              </th>
              {runtimeColumns.map((rt) => (
                <th key={rt.packageId} scope="col" className="px-3 py-2 font-medium">
                  {rt.packageName}
                </th>
              ))}
              <th scope="col" className="px-3 py-2 font-medium">
                Actions
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {projects.map((p) => (
              <tr key={p.id} className="align-top">
                <td className="px-3 py-2">
                  <div className="font-medium">{p.name}</div>
                  <div className="flex items-center gap-1 font-mono text-xs text-fg-muted">
                    <span className="break-all">{p.rootPath}</span>
                    <CopyButton value={p.rootPath} label={`folder of ${p.name}`} />
                  </div>
                  {p.detection.kind === "python" && <VenvHint project={p} />}
                  {terminal?.projectId === p.id && (
                    <div aria-live="polite" className="mt-1 text-xs">
                      {terminal.error ? (
                        <span className="text-danger">{terminal.error.message}</span>
                      ) : (
                        <span className="text-fg-muted">{terminal.message}</span>
                      )}
                    </div>
                  )}
                </td>
                {p.runtimes.map((rt) => (
                  <td key={rt.packageId} className="px-3 py-2">
                    <div>{runtimeLabel(rt)}</div>
                    <Button
                      className="mt-1"
                      onClick={() => setChanging({ project: p, packageId: rt.packageId })}
                      aria-label={`Change ${rt.packageName} for ${p.name}`}
                    >
                      Change
                    </Button>
                  </td>
                ))}
                <td className="px-3 py-2">
                  <div className="flex flex-wrap gap-2">
                    <Button onClick={() => openTerminal(p)} aria-label={`Open terminal for ${p.name}`}>
                      Open terminal
                    </Button>
                    <Button onClick={() => setRemoving(p)} aria-label={`Remove ${p.name} from Devluma`}>
                      Remove
                    </Button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {changing && (
        <ProjectRuntimeDialog
          project={changing.project}
          packageId={changing.packageId}
          onClose={() => setChanging(null)}
          onApplied={onChanged}
        />
      )}
      {removing && (
        <ConfirmChangeDialog
          open
          destructive
          onOpenChange={(open) => !open && setRemoving(null)}
          title={`Remove ${removing.name} from Devluma?`}
          target={removing.rootPath}
          impact="Devluma forgets this project and its runtime choices. The folder and every file in it stay exactly where they are."
          confirmLabel="Remove from Devluma"
          onConfirm={async () => {
            await ipc.removeProject(removing.id);
            onChanged();
          }}
        />
      )}
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
      <p className="mt-1 text-xs text-fg-muted">
        Virtual environment found — the project terminal uses it first ({project.detection.virtualEnvScripts}
        ).
      </p>
    );
  }
  const command = "python -m venv .venv";
  return (
    <p className="mt-1 flex flex-wrap items-center gap-1 text-xs text-fg-muted">
      No virtual environment yet. In the project terminal, run <code className="font-mono">{command}</code>
      <CopyButton value={command} label="venv command" /> — Devluma then puts it first on PATH so packages
      stay in the project.
    </p>
  );
}
