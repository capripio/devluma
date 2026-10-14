import { useState } from "react";
import { Link } from "react-router";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { formatBytes } from "../../lib/format";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { PackageView } from "../../types/dto";
import { RuntimeSwitchDialog } from "../runtimes/RuntimeSwitchDialog";
import { SettingsSection } from "./PreferenceSections";

const FOLDER_LABELS: Record<string, string> = {
  packages: "Downloaded tools",
  data: "Database data",
  backups: "Backups",
  services: "Service configs and logs",
  staging: "Unfinished downloads",
  temp: "Temporary files",
};

/** Settings → Storage: where Devluma keeps everything, and how much space it uses. */
export function StorageSettings() {
  const usage = useAsync(ipc.getStorageUsage);
  const [error, setError] = useState<AppError | null>(null);
  const open = (key: string) => {
    setError(null);
    ipc.openStorageFolder(key).catch((e: unknown) => setError(toAppError(e)));
  };

  return (
    <SettingsSection id="storage-heading" title="Storage">
      {usage.status === "pending" && <p role="status">Measuring Devluma&apos;s folders…</p>}
      {usage.status === "error" && <ErrorPanel error={usage.error} onRetry={usage.reload} />}
      {usage.status === "success" && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-fg-muted">Devluma folder</span>
            <span className="font-mono break-all">{usage.data.root}</span>
            <Button onClick={() => open("root")}>Open</Button>
          </div>
          <table className="w-full text-left">
            <caption className="sr-only">Space used by Devluma&apos;s folders</caption>
            <thead className="text-fg-muted">
              <tr>
                <th scope="col" className="py-1 font-medium">
                  Folder
                </th>
                <th scope="col" className="py-1 font-medium">
                  Size
                </th>
                <th scope="col" className="py-1 font-medium">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {usage.data.folders.map((f) => (
                <tr key={f.key} className="border-t border-border">
                  <td className="py-1.5">
                    {FOLDER_LABELS[f.key] ?? f.key}
                    <div className="font-mono text-xs break-all text-fg-muted">{f.path}</div>
                  </td>
                  <td className="py-1.5 whitespace-nowrap">
                    {formatBytes(f.bytes)}{" "}
                    <span className="text-fg-muted">
                      ({f.files} {f.files === 1 ? "file" : "files"})
                    </span>
                  </td>
                  <td className="py-1.5 text-right">
                    <Button
                      onClick={() => open(f.key)}
                      disabled={f.files === 0}
                      aria-label={`Open ${FOLDER_LABELS[f.key] ?? f.key} folder`}
                    >
                      Open
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="flex flex-wrap items-center gap-2">
            <Button onClick={usage.reload}>Measure again</Button>
          </div>
          <p className="text-fg-muted">
            To move everything to another folder, use &quot;Run setup again&quot; below. Database data and
            backups are never deleted automatically.
          </p>
        </>
      )}
      {error && <ErrorPanel error={error} />}
    </SettingsSection>
  );
}

/** Settings → Runtime defaults: the version used by projects without their own pin. */
export function RuntimeDefaultsSettings() {
  const catalog = useAsync(ipc.listPackages);
  const [changing, setChanging] = useState<{ pkg: PackageView; installationId: string } | null>(null);

  const runtimes =
    catalog.status === "success" ? catalog.data.packages.filter((p) => p.category === "runtime") : [];

  return (
    <SettingsSection id="runtime-defaults-heading" title="Runtime defaults">
      <p className="text-fg-muted">
        Projects use these versions unless they pin their own on the Projects page. Changing a default shows
        exactly what changes before anything is applied.
      </p>
      {catalog.status === "pending" && <p role="status">Loading runtimes…</p>}
      {catalog.status === "error" && <ErrorPanel error={catalog.error} onRetry={catalog.reload} />}
      {catalog.status === "success" && (
        <ul className="flex flex-col gap-3">
          {runtimes.map((pkg) => (
            <RuntimeDefaultRow
              key={pkg.id}
              pkg={pkg}
              onChange={(installationId) => setChanging({ pkg, installationId })}
            />
          ))}
        </ul>
      )}
      {changing && (
        <RuntimeSwitchDialog
          title={`Change the default ${changing.pkg.displayName}?`}
          request={{ packageId: changing.pkg.id, scope: "global", installationId: changing.installationId }}
          confirmLabel="Make default"
          onClose={() => setChanging(null)}
          onApplied={() => {
            setChanging(null);
            catalog.refresh();
          }}
        />
      )}
    </SettingsSection>
  );
}

function RuntimeDefaultRow({
  pkg,
  onChange,
}: {
  pkg: PackageView;
  onChange: (installationId: string) => void;
}) {
  const installed = pkg.versions.filter((v) => v.installation?.status === "installed");
  const current = installed.find((v) => v.installation?.isGlobalDefault);
  const [choice, setChoice] = useState(current?.installation?.id ?? installed[0]?.installation?.id ?? "");
  const selectId = `default-${pkg.id}`;

  if (installed.length === 0) {
    return (
      <li className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{pkg.displayName}</span>
        <span className="text-fg-muted">Not installed.</span>
        <Link to={`/runtimes/${pkg.id}`} className="text-accent hover:underline">
          Install
        </Link>
      </li>
    );
  }
  return (
    <li className="flex flex-wrap items-center gap-2">
      <label htmlFor={selectId} className="w-24 font-medium">
        {pkg.displayName}
      </label>
      <select
        id={selectId}
        value={choice}
        onChange={(e) => setChoice(e.target.value)}
        className="rounded-md border border-border bg-surface px-2 py-1.5"
      >
        {installed.map((v) => (
          <option key={v.key} value={v.installation!.id}>
            {v.version}
            {v.installation!.isGlobalDefault ? " (current default)" : ""}
          </option>
        ))}
      </select>
      <Button
        onClick={() => onChange(choice)}
        disabled={!choice || choice === current?.installation?.id}
        aria-label={`Make the selected ${pkg.displayName} version the default`}
      >
        Make default
      </Button>
      {!current && <span className="text-fg-muted">No default yet.</span>}
    </li>
  );
}

/** Settings → Security & privacy: what Devluma does and doesn't do, and where to control it. */
export function SecuritySettings() {
  return (
    <SettingsSection id="security-heading" title="Security & privacy">
      <ul className="flex list-inside list-disc flex-col gap-1.5 text-fg-muted">
        <li>
          Devluma doesn&apos;t collect telemetry or usage data. There is nothing to turn off. It checks for
          updates only after you set an update address.
        </li>
        <li>
          Services listen on 127.0.0.1 only, so other computers on your network can&apos;t reach them. A site
          is reachable from the internet only while you share it with an ngrok tunnel.
        </li>
        <li>
          Downloads are checked against the catalog&apos;s SHA-256 before they are unpacked. Catalog and app
          updates must be signed with Devluma&apos;s key.
        </li>
        <li>
          The ngrok authtoken and the local CA&apos;s key are encrypted for your Windows account. Logs and
          diagnostic exports mask secrets.
        </li>
        <li>
          Administrator rights are requested only for writing the hosts file, and only after you&apos;ve seen
          the exact change.
        </li>
      </ul>
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        <Link to="/ssl" className="text-accent hover:underline">
          Local certificate authority
        </Link>
        <Link to="/hosts" className="text-accent hover:underline">
          Hosts file
        </Link>
        <Link to="/logs" className="text-accent hover:underline">
          Logs &amp; diagnostics
        </Link>
      </div>
    </SettingsSection>
  );
}

/** Settings → About. */
export function AboutSettings() {
  const info = useAsync(ipc.getAppInfo);
  return (
    <SettingsSection id="about-heading" title="About">
      {info.status === "pending" && <p role="status">Loading…</p>}
      {info.status === "error" && <ErrorPanel error={info.error} onRetry={info.reload} />}
      {info.status === "success" && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
          <dt className="text-fg-muted">Version</dt>
          <dd>
            {info.data.version}
            {info.data.debugBuild ? " (development build)" : ""}
          </dd>
          <dt className="text-fg-muted">Platform</dt>
          <dd>
            {info.data.os} {info.data.arch}
          </dd>
          <dt className="text-fg-muted">License</dt>
          <dd>GNU General Public License v3.0</dd>
        </dl>
      )}
      <p className="text-fg-muted">
        Tools Devluma downloads for you (PHP, Nginx, databases, Cmder and others) aren&apos;t part of Devluma.
        Each keeps its own license, shown on its page under Runtimes &amp; Packages.
      </p>
    </SettingsSection>
  );
}
