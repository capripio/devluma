import { useState, type FormEvent } from "react";
import { Link } from "react-router";
import { Button } from "../../components/Button";
import { DiffViewer } from "../../components/DiffViewer";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PermissionExplainer } from "../../components/PermissionExplainer";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ProjectView, SitePlan, SiteRequest, SiteView } from "../../types/dto";

const PHP_KINDS = new Set(["laravel", "symfony", "wordpress", "php"]);

/** "My Shop" → "my-shop.test" (mirrors `domain_name::suggest`). */
export function suggestDomain(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `${slug || "site"}.test`;
}

interface SiteFormProps {
  /** Present when re-applying an existing site. */
  existing?: SiteView;
  onDone: () => void;
}

/** Add or re-apply a site: fill in → preview the exact changes → apply. */
export function SiteForm({ existing, onDone }: SiteFormProps) {
  const data = useAsync(() => Promise.all([ipc.listProjects(), ipc.listServices()]));
  if (data.status === "pending") {
    return (
      <p role="status" className="text-sm text-fg-muted">
        Loading projects and web servers…
      </p>
    );
  }
  if (data.status === "error") return <ErrorPanel error={data.error} onRetry={data.reload} />;
  const [projects, services] = data.data;
  const webServers = services.filter((s) => s.lifecycleAdapter === "nginx");
  if (projects.length === 0 || webServers.length === 0) {
    return (
      <div className="flex flex-col items-start gap-2 rounded-lg border border-border p-4 text-sm">
        {projects.length === 0 && (
          <p>
            Add a project first on the{" "}
            <Link to="/projects" className="text-accent hover:underline">
              Projects
            </Link>{" "}
            page.
          </p>
        )}
        {webServers.length === 0 && (
          <p>
            Create an Nginx service first on the{" "}
            <Link to="/services" className="text-accent hover:underline">
              Services
            </Link>{" "}
            page.
          </p>
        )}
        <Button onClick={onDone}>Close</Button>
      </div>
    );
  }
  return <Form projects={projects} webServers={webServers} existing={existing} onDone={onDone} />;
}

function Form({
  projects,
  webServers,
  existing,
  onDone,
}: {
  projects: ProjectView[];
  webServers: { id: string; instanceName: string; port: number | null }[];
  existing?: SiteView;
  onDone: () => void;
}) {
  const initialProject = projects.find((p) => p.id === existing?.projectId) ?? projects[0]!;
  const [projectId, setProjectId] = useState(initialProject.id);
  const project = projects.find((p) => p.id === projectId) ?? initialProject;
  const [domain, setDomain] = useState(existing?.domain ?? suggestDomain(initialProject.name));
  const [webId, setWebId] = useState(existing?.webServerServiceId ?? webServers[0]!.id);
  const [usePhp, setUsePhp] = useState(
    existing ? existing.phpInstallationId !== null : PHP_KINDS.has(initialProject.detection.kind ?? ""),
  );
  const [docroot, setDocroot] = useState(
    existing?.documentRoot ?? initialProject.detection.suggestedDocumentRoot,
  );
  const [manageHosts, setManageHosts] = useState<boolean>(true);
  const [https, setHttps] = useState<boolean>(existing?.sslEnabled ?? false);
  const [plan, setPlan] = useState<SitePlan | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const request = (): SiteRequest => ({
    ...(existing || plan ? { siteId: existing?.id ?? plan?.siteId } : {}),
    projectId,
    domain,
    webServerServiceId: webId,
    usePhp,
    documentRoot: docroot,
    manageHosts,
    https,
  });

  const changeProject = (id: string) => {
    const p = projects.find((x) => x.id === id);
    setProjectId(id);
    setPlan(null);
    if (p && !existing) {
      setDomain(suggestDomain(p.name));
      setDocroot(p.detection.suggestedDocumentRoot);
      setUsePhp(PHP_KINDS.has(p.detection.kind ?? ""));
    }
  };

  const preview = async (e: FormEvent) => {
    e.preventDefault();
    setPending(true);
    setError(null);
    try {
      setPlan(await ipc.previewSite(request()));
    } catch (err) {
      setPlan(null);
      setError(toAppError(err));
    } finally {
      setPending(false);
    }
  };

  const apply = async () => {
    if (!plan || pending) return;
    setPending(true);
    setError(null);
    try {
      await ipc.applySite({
        ...request(),
        siteId: plan.siteId,
        ...(plan.hosts ? { expectedHostsSha256: plan.hosts.sha256Before } : {}),
      });
      onDone();
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setPending(false);
    }
  };

  // Any edit invalidates the preview.
  const edit =
    <T,>(set: (v: T) => void) =>
    (v: T) => {
      set(v);
      setPlan(null);
    };

  const input = "rounded-md border border-border bg-surface px-2 py-1.5";
  return (
    <form
      onSubmit={preview}
      aria-label={existing ? `Re-apply ${existing.domain}` : "Add site"}
      className="flex flex-col gap-3 rounded-lg border border-border p-4 text-sm"
    >
      <div className="flex flex-wrap gap-4">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Project</span>
          <select
            value={projectId}
            onChange={(e) => changeProject(e.target.value)}
            className={input}
            disabled={Boolean(existing)}
          >
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name} ({p.detection.label})
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Domain</span>
          <input
            required
            value={domain}
            onChange={(e) => edit(setDomain)(e.target.value)}
            className={`${input} w-56 font-mono`}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Web server</span>
          <select value={webId} onChange={(e) => edit(setWebId)(e.target.value)} className={input}>
            {webServers.map((w) => (
              <option key={w.id} value={w.id}>
                {w.instanceName} (port {w.port})
              </option>
            ))}
          </select>
        </label>
      </div>
      <label className="flex flex-col gap-1">
        <span className="text-fg-muted">Document root (inside {project.rootPath})</span>
        <input
          required
          value={docroot}
          onChange={(e) => edit(setDocroot)(e.target.value)}
          className={`${input} font-mono`}
        />
      </label>
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={usePhp} onChange={(e) => edit(setUsePhp)(e.target.checked)} />
        Serve PHP with this project&apos;s PHP version
      </label>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={manageHosts}
          onChange={(e) => edit(setManageHosts)(e.target.checked)}
        />
        Add the domain to the Windows hosts file
      </label>
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={https} onChange={(e) => edit(setHttps)(e.target.checked)} />
        Also serve over HTTPS with a local development certificate
      </label>

      {error && <ErrorPanel error={error} />}

      {plan && (
        <section aria-label="Site preview" className="flex flex-col gap-3 rounded-md bg-surface-raised p-3">
          <p>
            <span className="font-medium">{plan.url}</span> →{" "}
            <span className="font-mono">{plan.documentRoot}</span>
            {plan.php && (
              <>
                {" "}
                · PHP {plan.php.version} (FastCGI on 127.0.0.1:{plan.php.port})
              </>
            )}
          </p>
          {plan.https && (
            <div className="flex flex-col gap-1">
              <p>
                HTTPS: <span className="font-medium">{plan.https.url}</span> ·{" "}
                {plan.https.certificateId
                  ? "uses the existing certificate for this domain"
                  : "a certificate is created by Devluma's local CA"}
              </p>
              {!plan.https.caTrusted && (
                <p className="text-warning">
                  Browsers will show a warning until you trust Devluma&apos;s local CA on the SSL Certificates
                  page. Applying this site doesn&apos;t trust anything.
                </p>
              )}
            </div>
          )}
          {plan.warnings.length > 0 && (
            <ul aria-label="Warnings" className="list-inside list-disc text-warning">
              {plan.warnings.map((w) => (
                <li key={w}>{w}</li>
              ))}
            </ul>
          )}
          <DiffViewer diff={plan.configDiff} label="Site configuration changes" />
          {plan.hosts && !plan.hosts.unchanged && (
            <>
              <DiffViewer diff={plan.hosts.diff} label="Hosts file changes" />
              {plan.hosts.needsElevation && (
                <PermissionExplainer>
                  Only Devluma&apos;s own section of <span className="font-mono">{plan.hosts.path}</span>{" "}
                  changes: the line shown above. A backup is taken first and the file is checked after
                  writing.
                </PermissionExplainer>
              )}
            </>
          )}
          {plan.hosts?.unchanged && <p className="text-fg-muted">The hosts file already has this entry.</p>}
        </section>
      )}

      <div className="flex gap-2">
        {plan ? (
          <Button variant="primary" pending={pending} onClick={apply}>
            Apply
          </Button>
        ) : (
          <Button type="submit" variant="primary" pending={pending}>
            Preview changes
          </Button>
        )}
        <Button onClick={onDone} disabled={pending}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
