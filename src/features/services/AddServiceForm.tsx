import { useState, type FormEvent } from "react";
import { Link } from "react-router";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { PackageView } from "../../types/dto";
import { servicePackageInfo } from "./servicePackages";

const SERVICE_CATEGORIES: PackageView["category"][] = ["web_server", "database", "mail"];

interface Choice {
  installationId: string;
  label: string;
  category: PackageView["category"];
  packageId: string;
}

/** Creates a service from an installed web server, database or mail catcher. */
export function AddServiceForm({ onDone }: { onDone: () => void }) {
  const catalog = useAsync(ipc.listPackages);
  // Default ports from Settings; the built-in defaults apply if they can't be loaded.
  const prefs = useAsync(ipc.getPreferences);
  const choices: Choice[] =
    catalog.status === "success"
      ? catalog.data.packages
          .filter((p) => SERVICE_CATEGORIES.includes(p.category))
          .flatMap((p) =>
            p.versions
              .filter((v) => v.installation?.status === "installed")
              .map((v) => ({
                installationId: v.installation!.id,
                label: `${p.displayName} ${v.version}`,
                category: p.category,
                packageId: p.id,
              })),
          )
      : [];

  const [installationId, setInstallationId] = useState("");
  const selected = choices.find((c) => c.installationId === installationId) ?? choices[0];
  const isDb = selected?.category === "database";
  const [name, setName] = useState("");
  const [port, setPort] = useState("");
  const [adminPort, setAdminPort] = useState("");
  const [dataPath, setDataPath] = useState("");
  const [useExisting, setUseExisting] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  if (catalog.status === "pending") {
    return (
      <p role="status" className="text-sm text-fg-muted">
        Loading installed packages…
      </p>
    );
  }
  if (catalog.status === "error") return <ErrorPanel error={catalog.error} onRetry={catalog.reload} />;
  if (!selected) {
    return (
      <div className="flex flex-col items-start gap-2 rounded-lg border border-border p-4 text-sm">
        <p>No web server, database or Mailpit is installed yet.</p>
        <Link to="/runtimes" className="text-accent hover:underline">
          Install one from Runtimes &amp; Packages
        </Link>
        <Button onClick={onDone}>Close</Button>
      </div>
    );
  }

  const info = servicePackageInfo(
    selected.packageId,
    prefs.status === "success" ? prefs.data.preferences.defaultPorts : undefined,
  );
  const effectivePort = port === "" ? info.defaultPort : Number(port);
  const effectiveAdminPort =
    info.adminPort && (adminPort === "" ? info.adminPort.default : Number(adminPort));
  const effectiveName = name.trim() || info.defaultName;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await ipc.createService({
        installationId: selected.installationId,
        name: effectiveName,
        port: effectivePort,
        ...(effectiveAdminPort ? { adminPort: effectiveAdminPort } : {}),
        ...(isDb && dataPath.trim() ? { dataPath: dataPath.trim() } : {}),
        useExistingData: isDb && useExisting,
      });
      onDone();
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
      aria-label="Add service"
      className="flex flex-col gap-3 rounded-lg border border-border p-4 text-sm"
    >
      <div className="flex flex-wrap gap-4">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Installed version</span>
          <select
            value={selected.installationId}
            onChange={(e) => setInstallationId(e.target.value)}
            className={input}
          >
            {choices.map((c) => (
              <option key={c.installationId} value={c.installationId}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Name</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={effectiveName}
            pattern="[A-Za-z0-9_\-]{1,60}"
            title="Letters, digits, '-' or '_'"
            className={`${input} w-40`}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Port</span>
          <input
            type="number"
            min={1}
            max={65535}
            value={port}
            onChange={(e) => setPort(e.target.value)}
            placeholder={String(info.defaultPort)}
            className={`${input} w-28 font-mono`}
          />
        </label>
        {info.adminPort && (
          <label className="flex flex-col gap-1">
            <span className="text-fg-muted">{info.adminPort.label}</span>
            <input
              type="number"
              min={1}
              max={65535}
              value={adminPort}
              onChange={(e) => setAdminPort(e.target.value)}
              placeholder={String(info.adminPort.default)}
              className={`${input} w-28 font-mono`}
            />
          </label>
        )}
      </div>
      {info.note && <p className="text-fg-muted">{info.note}</p>}
      {isDb && (
        <>
          <label className="flex flex-col gap-1">
            <span className="text-fg-muted">Data folder (optional)</span>
            <input
              value={dataPath}
              onChange={(e) => setDataPath(e.target.value)}
              placeholder={`Default: Devluma's data folder\\${selected.packageId}\\${effectiveName}`}
              className={`${input} font-mono`}
            />
          </label>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={useExisting} onChange={(e) => setUseExisting(e.target.checked)} />
            Use the existing data in this folder (don't initialize)
          </label>
          <p className="text-fg-muted">{info.dataNote} Existing data is never initialized or overwritten.</p>
        </>
      )}
      {error && <ErrorPanel error={error} />}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" pending={pending}>
          Create service
        </Button>
        <Button onClick={onDone} disabled={pending}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
