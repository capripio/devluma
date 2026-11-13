import { useState, type FormEvent } from "react";
import { useStartupStatus } from "../../app/startup";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useServices } from "../services/useServices";
import { slugify } from "../sites/SiteForm";
import { useNewSite } from "./NewSite";
import { defaultServiceIds, ServiceChecklist } from "./ProjectServices";

/**
 * New project in `www\<folder>` (new, or a folder already there) with its website, created
 * right away (ADR 0020). Web apps have their own installer (`WebAppInstaller`).
 */
export function AddProjectForm({
  onCancel,
  onAdded,
}: {
  onCancel: () => void;
  onAdded: (projectId: string, note: string | null) => void;
}) {
  const projectsDir = useStartupStatus()?.projectsDir ?? "www";
  const [name, setName] = useState("");
  // The folder follows the name until the user edits it.
  const [folder, setFolder] = useState<string | null>(null);
  const services = useServices();
  // Preselected once the services load: the first of each kind; `null` until the user edits.
  const [serviceIds, setServiceIds] = useState<string[] | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const serviceList = services.status === "success" ? services.data : [];
  const serviceIdsValue = serviceIds ?? defaultServiceIds(serviceList);
  const folderValue = folder ?? slugify(name);
  const { site, fields } = useNewSite(name, serviceList);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      const created = await ipc.createProject(name, folderValue, serviceIdsValue, site ?? null);
      onAdded(
        created.project.id,
        created.siteError
          ? `${created.project.name} was added, but its site wasn't created: ${created.siteError}`
          : created.siteOperationId && site
            ? `Creating ${site.domain}…`
            : null,
      );
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

      {fields(false)}

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
