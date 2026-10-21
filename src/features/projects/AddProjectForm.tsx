import { useState, type FormEvent } from "react";
import { useStartupStatus } from "../../app/startup";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { Project } from "../../types/dto";
import { useServices } from "../services/useServices";
import { slugify, suggestDomain } from "../sites/SiteForm";
import { defaultServiceIds, ServiceChecklist } from "./ProjectServices";

/** New project in `www\<folder>`: name → folder and domain follow it; services preselected. */
export function AddProjectForm({
  onCancel,
  onAdded,
}: {
  onCancel: () => void;
  /** `domain` is set when the user also wants a site for the project. */
  onAdded: (project: Project, domain: string | null) => void;
}) {
  const projectsDir = useStartupStatus()?.projectsDir ?? "www";
  // The domain suffix from Settings; ".test" if preferences can't be read.
  const suffix = useAsync(() =>
    ipc.getPreferences().then(
      (p) => p.preferences.domainSuffix,
      () => "test",
    ),
  );
  const [name, setName] = useState("");
  // Folder and domain follow the name until the user edits them.
  const [folder, setFolder] = useState<string | null>(null);
  const [domain, setDomain] = useState<string | null>(null);
  const [createSite, setCreateSite] = useState(true);
  const services = useServices();
  // Preselected once the services load: the first of each kind; `null` until the user edits.
  const [serviceIds, setServiceIds] = useState<string[] | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const serviceIdsValue =
    serviceIds ?? (services.status === "success" ? defaultServiceIds(services.data) : []);
  const folderValue = folder ?? slugify(name);
  const domainValue =
    domain ?? (name.trim() ? suggestDomain(name, suffix.status === "success" ? suffix.data : "test") : "");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      const project = await ipc.createProject(name, folderValue, serviceIdsValue);
      onAdded(project, createSite ? domainValue : null);
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
      className="flex flex-col gap-4 rounded-md border border-border bg-surface-raised p-4 text-sm"
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
            {projectsDir}\{folderValue || "…"} — created if it doesn&apos;t exist
          </span>
        </div>
      </div>
      <div className="flex flex-wrap items-end gap-4">
        <label className="flex items-center gap-2 py-1.5">
          <input type="checkbox" checked={createSite} onChange={(e) => setCreateSite(e.target.checked)} />
          Also create a site at
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
      </div>
      {services.status === "success" && (
        <ServiceChecklist
          services={services.data}
          selected={serviceIdsValue}
          onChange={setServiceIds}
          legend={'Services this project uses ("Start all" starts them)'}
        />
      )}
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
