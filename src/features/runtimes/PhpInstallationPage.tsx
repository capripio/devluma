import { ArrowLeft, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { Link, useParams } from "react-router";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import type { PhpDetails } from "../../types/dto";

type Template = "development" | "production";

const TEMPLATE_LABELS: Record<Template, string> = {
  development: "development",
  production: "production",
};

export function PhpInstallationPage() {
  const { installationId = "" } = useParams();
  const [details, setDetails] = useState<PhpDetails | null>(null);
  const loaded = useAsync(() => ipc.getPhpDetails(installationId));
  const current = details ?? (loaded.status === "success" ? loaded.data : null);

  return (
    <>
      <Link to="/runtimes/php" className="inline-flex items-center gap-1 text-sm text-accent hover:underline">
        <ArrowLeft aria-hidden className="size-4" />
        PHP
      </Link>
      <PageHeader
        title="PHP installation"
        description="Configuration and extensions for this installed version."
      />
      {loaded.status === "pending" && !current && (
        <p role="status" className="text-sm text-fg-muted">
          Inspecting PHP…
        </p>
      )}
      {loaded.status === "error" && <ErrorPanel error={loaded.error} onRetry={loaded.reload} />}
      {current && <Details details={current} installationId={installationId} onUpdated={setDetails} />}
    </>
  );
}

function PathRow({ label, value }: { label: string; value: string }) {
  return (
    <>
      <dt className="text-fg-muted">{label}</dt>
      <dd className="flex min-w-0 items-center gap-1 font-mono text-xs break-all">
        {value}
        <CopyButton value={value} label={label.toLowerCase()} />
      </dd>
    </>
  );
}

function Details({
  details: d,
  installationId,
  onUpdated,
}: {
  details: PhpDetails;
  installationId: string;
  onUpdated: (d: PhpDetails) => void;
}) {
  const [creating, setCreating] = useState<Template | null>(null);
  const loaded = new Set(d.loadedModules.map((m) => m.toLowerCase()));

  return (
    <>
      <dl className="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2 text-sm">
        <PathRow label="Install folder" value={d.installPath} />
        <PathRow label="php.ini" value={d.iniPath} />
        <PathRow label="Extension folder" value={d.extensionDir} />
      </dl>

      {d.startupWarnings.length > 0 && (
        <section
          aria-label="PHP startup warnings"
          className="flex gap-3 rounded-md border border-warning/40 bg-warning-subtle p-4 text-sm"
        >
          <TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning" />
          <ul className="flex flex-col gap-1 font-mono text-xs text-fg">
            {d.startupWarnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        </section>
      )}

      <section
        aria-labelledby="ini-heading"
        className="flex flex-col gap-2 rounded-md border border-border p-4 text-sm"
      >
        <h2 id="ini-heading" className="font-semibold">
          php.ini
        </h2>
        {d.iniExists ? (
          <p className="text-fg-muted">
            This version uses the php.ini above. Edit it in your editor; managed extension toggles arrive in a
            later update.
          </p>
        ) : (
          <>
            <p className="text-fg-muted">
              No php.ini yet, so PHP runs with built-in defaults and no extensions enabled. Create one from an
              official template:
            </p>
            <div className="flex flex-wrap gap-2">
              {d.availableTemplates.map((t) => (
                <Button key={t} onClick={() => setCreating(t)}>
                  Create from {TEMPLATE_LABELS[t]} template
                </Button>
              ))}
              {d.availableTemplates.length === 0 && (
                <p className="text-fg-muted">This installation doesn't include php.ini templates.</p>
              )}
            </div>
          </>
        )}
      </section>

      <section aria-labelledby="ext-heading" className="flex flex-col gap-2 text-sm">
        <h2 id="ext-heading" className="font-semibold">
          Extensions
        </h2>
        <p className="text-fg-muted">
          {d.loadedModules.length} modules loaded · {d.availableExtensions.length} extension DLLs available
        </p>
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(10rem,1fr))] gap-1">
          {d.availableExtensions.map((e) => (
            <li key={e} className="flex items-center gap-2 font-mono text-xs">
              <span
                className={`rounded px-1.5 py-0.5 ${loaded.has(e) ? "bg-success-subtle text-success" : "bg-neutral-subtle text-neutral"}`}
              >
                {loaded.has(e) ? "On" : "Off"}
              </span>
              {e}
            </li>
          ))}
        </ul>
      </section>

      {creating && (
        <ConfirmChangeDialog
          open
          onOpenChange={(open) => !open && setCreating(null)}
          title={`Create php.ini from the ${TEMPLATE_LABELS[creating]} template?`}
          target={d.iniPath}
          impact={
            <ul className="list-inside list-disc space-y-1">
              <li>Copies PHP's official php.ini-{creating} template.</li>
              <li>
                Sets <span className="font-mono">extension_dir</span> to this installation's own extension
                folder, so extensions never load from another PHP.
              </li>
              <li>PHP checks the new file before it's saved. Only this PHP version is affected.</li>
            </ul>
          }
          rollback="An existing php.ini is never overwritten. Delete the file to go back to built-in defaults."
          confirmLabel="Create php.ini"
          onConfirm={async () => onUpdated(await ipc.createPhpIni(installationId, creating))}
        />
      )}
    </>
  );
}
