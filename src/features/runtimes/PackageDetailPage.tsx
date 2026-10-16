import { ArrowLeft } from "lucide-react";
import { useState } from "react";
import { Link, useParams } from "react-router";
import { isActive, useOnOperationsSettled, useOperations } from "../../app/operations";
import { Button } from "../../components/Button";
import { CancelOperationButton } from "../../components/CancelOperationButton";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { OperationStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { CATEGORY_LABELS, formatBytes } from "../../lib/format";
import { useAsync } from "../../lib/useAsync";
import type { OperationProgress, PackageView, VersionView } from "../../types/dto";
import { INSTALL_OPERATION } from "./packages";
import { UninstallDialog } from "./UninstallDialog";
import { RuntimeSwitchDialog } from "./RuntimeSwitchDialog";
import { LoadingState } from "../../components/LoadingState";

/** Download + extracted copy + headroom; mirrors `DISK_FACTOR` in `services/install.rs`. */
const DISK_FACTOR = 4;

export function PackageDetailPage() {
  const { packageId } = useParams();
  const catalog = useAsync(ipc.listPackages);
  useOnOperationsSettled((op) => op.operationType === INSTALL_OPERATION, catalog.refresh);

  const back = (
    <Link to="/runtimes" className="inline-flex items-center gap-1 text-sm text-accent hover:underline">
      <ArrowLeft aria-hidden className="size-4" />
      Runtimes & Packages
    </Link>
  );

  if (catalog.status === "pending") {
    return (
      <>
        {back}
        <LoadingState label="Loading package…" shape="panel" rows={4} />
      </>
    );
  }
  if (catalog.status === "error") {
    return (
      <>
        {back}
        <ErrorPanel error={catalog.error} onRetry={catalog.reload} />
      </>
    );
  }
  const pkg = catalog.data.packages.find((p) => p.id === packageId);
  if (!pkg) {
    return (
      <>
        {back}
        <PageHeader title="Package not found" description={`"${packageId}" isn't in the current catalog.`} />
      </>
    );
  }
  return (
    <>
      {back}
      <PageHeader title={pkg.displayName} description={pkg.description ?? undefined} />
      <PackageFacts pkg={pkg} />
      <VersionsTable pkg={pkg} onChanged={catalog.refresh} />
    </>
  );
}

function PackageFacts({ pkg }: { pkg: PackageView }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm">
      <dt className="text-fg-muted">Category</dt>
      <dd>{CATEGORY_LABELS[pkg.category]}</dd>
      <dt className="text-fg-muted">Publisher</dt>
      <dd>{pkg.publisher}</dd>
      <dt className="text-fg-muted">License</dt>
      <dd>
        {pkg.license.id}
        {pkg.license.url && (
          <span className="ml-2 font-mono text-xs text-fg-muted select-all">{pkg.license.url}</span>
        )}
      </dd>
      {pkg.homepageUrl && (
        <>
          <dt className="text-fg-muted">Homepage</dt>
          <dd className="font-mono text-xs select-all">{pkg.homepageUrl}</dd>
        </>
      )}
    </dl>
  );
}

function VersionsTable({ pkg, onChanged }: { pkg: PackageView; onChanged: () => void }) {
  const { operations } = useOperations();
  const [confirming, setConfirming] = useState<VersionView | null>(null);
  const [makingDefault, setMakingDefault] = useState<VersionView | null>(null);
  const [uninstalling, setUninstalling] = useState<VersionView | null>(null);
  const latestOp = (v: VersionView): OperationProgress | undefined =>
    operations.find((op) => op.operationType === INSTALL_OPERATION && op.targetId === v.key);

  return (
    <section aria-labelledby="versions-heading" className="flex flex-col gap-2">
      <h2 id="versions-heading" className="text-sm font-semibold">
        Versions
      </h2>
      {pkg.versions.length === 0 ? (
        <p className="text-sm text-fg-muted">No installable versions are listed for this package.</p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full text-left text-sm">
            <thead className="bg-surface-raised text-fg-muted">
              <tr>
                {["Version", "Architecture", "Download", "Source", "Support", "Status", "Action"].map((h) => (
                  <th key={h} scope="col" className="px-3 py-2 font-medium">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {pkg.versions.map((v) => (
                <VersionRow
                  key={v.key}
                  pkg={pkg}
                  version={v}
                  op={latestOp(v)}
                  onInstall={() => setConfirming(v)}
                  onMakeDefault={() => setMakingDefault(v)}
                  onUninstall={() => setUninstalling(v)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {confirming && <InstallDialog pkg={pkg} version={confirming} onClose={() => setConfirming(null)} />}
      {uninstalling?.installation && (
        <UninstallDialog
          installationId={uninstalling.installation.id}
          label={`${pkg.displayName} ${uninstalling.version}`}
          onClose={() => setUninstalling(null)}
          onDone={onChanged}
        />
      )}
      {makingDefault?.installation && (
        <RuntimeSwitchDialog
          title={`Make ${pkg.displayName} ${makingDefault.version} the default?`}
          request={{ packageId: pkg.id, scope: "global", installationId: makingDefault.installation.id }}
          confirmLabel="Make default"
          onClose={() => setMakingDefault(null)}
          onApplied={onChanged}
        />
      )}
    </section>
  );
}

function VersionRow({
  pkg,
  version: v,
  op,
  onInstall,
  onMakeDefault,
  onUninstall,
}: {
  pkg: PackageView;
  version: VersionView;
  op: OperationProgress | undefined;
  onInstall: () => void;
  onMakeDefault: () => void;
  onUninstall: () => void;
}) {
  const running = op && isActive(op);
  const status = v.installation?.status;
  const reasonId = `unavailable-${v.key.replace(/\W+/g, "-")}`;

  return (
    <tr className="align-top">
      <td className="px-3 py-2 font-mono">{v.version}</td>
      <td className="px-3 py-2">{v.architecture}</td>
      <td className="px-3 py-2 whitespace-nowrap">{formatBytes(v.sizeBytes)}</td>
      <td className="px-3 py-2 font-mono text-xs">{v.sourceHost}</td>
      <td className="px-3 py-2 text-xs text-fg-muted">
        {v.releaseDate && <div>Released {v.releaseDate}</div>}
        {v.endOfLifeDate && <div>End of life {v.endOfLifeDate}</div>}
      </td>
      <td className="px-3 py-2">
        {running ? (
          <div className="flex flex-col gap-1">
            <OperationStatusBadge status={op.status} />
            <span className="text-xs text-fg-muted">{op.phase}</span>
            {op.current !== undefined && op.total !== undefined && (
              <progress
                max={op.total}
                value={op.current}
                className="w-28"
                aria-label={`Download progress for ${pkg.displayName} ${v.version}`}
              />
            )}
          </div>
        ) : status === "installed" ? (
          <span className="flex flex-col gap-1">
            <span>Installed{v.installation?.isGlobalDefault ? " (default)" : ""}</span>
            <span className="flex items-center gap-1 font-mono text-xs text-fg-muted">
              <span className="break-all">{v.installation?.installPath}</span>
              <CopyButton value={v.installation?.installPath ?? ""} label="install path" />
            </span>
          </span>
        ) : status === "broken" ? (
          <span className="text-warning">Broken — incomplete or files missing</span>
        ) : v.unavailableReason ? (
          <span id={reasonId} className="text-fg-muted">
            {v.unavailableReason}
          </span>
        ) : op?.status === "failed" ? (
          <span className="text-danger">Last attempt failed: {op.safeMessage}</span>
        ) : (
          <span className="text-fg-muted">Not installed</span>
        )}
      </td>
      <td className="px-3 py-2">
        {running ? (
          <CancelOperationButton
            operationId={op.operationId}
            label={`Cancel installing ${pkg.displayName} ${v.version}`}
          />
        ) : status === "installed" && v.installation ? (
          <div className="flex flex-wrap gap-2">
            {!v.installation.isGlobalDefault && (
              <Button onClick={onMakeDefault} aria-label={`Make ${pkg.displayName} ${v.version} the default`}>
                Make default
              </Button>
            )}
            <Button onClick={onUninstall} aria-label={`Uninstall ${pkg.displayName} ${v.version}`}>
              Uninstall
            </Button>
            {pkg.id === "php" && (
              <Link
                to={`/runtimes/php/installations/${v.installation.id}`}
                aria-label={`Details for ${pkg.displayName} ${v.version}`}
                className="inline-flex items-center rounded-md border border-border px-3 py-1.5 text-sm hover:bg-surface-sunken"
              >
                Details
              </Link>
            )}
          </div>
        ) : (
          <Button
            variant="primary"
            onClick={onInstall}
            disabled={Boolean(v.unavailableReason)}
            aria-describedby={v.unavailableReason ? reasonId : undefined}
            aria-label={`${status === "broken" || op?.status === "failed" ? "Retry installing" : "Install"} ${pkg.displayName} ${v.version}`}
          >
            {status === "broken" || op?.status === "failed" ? "Retry" : "Install"}
          </Button>
        )}
      </td>
    </tr>
  );
}

function InstallDialog({
  pkg,
  version: v,
  onClose,
}: {
  pkg: PackageView;
  version: VersionView;
  onClose: () => void;
}) {
  return (
    <ConfirmChangeDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={`Install ${pkg.displayName} ${v.version}?`}
      target={v.installPath}
      impact={
        <ul className="list-inside list-disc space-y-1">
          <li>
            Downloads {formatBytes(v.sizeBytes)} from <span className="font-mono">{v.sourceHost}</span>.
          </li>
          <li>Needs about {formatBytes(v.sizeBytes * DISK_FACTOR)} of free disk space while installing.</li>
          <li>
            Checked against SHA-256 <span className="font-mono text-xs break-all">{v.sha256}</span> and by
            running its executables before anything is marked installed.
          </li>
          <li>No administrator permission is needed.</li>
        </ul>
      }
      rollback="Only the folder above is created. Other installed versions and your current default don't change. If anything fails, downloaded files are removed."
      confirmLabel="Install"
      onConfirm={async () => {
        await ipc.installPackage({ packageId: pkg.id, version: v.version, architecture: v.architecture });
      }}
    />
  );
}
