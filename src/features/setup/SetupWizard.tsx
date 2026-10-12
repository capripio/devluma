import { useState } from "react";
import { useOperations } from "../../app/operations";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { formatBytes } from "../../lib/format";
import { useAsync } from "../../lib/useAsync";
import type { PackageView, SetupStatus } from "../../types/dto";

type Step = "welcome" | "storage" | "stack" | "review" | "running";

/** Packages the wizard offers, grouped as the user thinks about them. */
const GROUPS: { title: string; ids: string[]; pickOne?: boolean }[] = [
  { title: "Language runtimes", ids: ["php", "node", "python"] },
  { title: "Web server", ids: ["nginx", "apache"], pickOne: true },
  { title: "Database", ids: ["mariadb", "postgresql", "mongodb"] },
  { title: "Tools", ids: ["mailpit"] },
];

const PRESETS: { id: string; label: string; description: string; packages: string[] }[] = [
  {
    id: "php",
    label: "PHP web stack",
    description: "PHP, Nginx and MariaDB — for Laravel, WordPress, Symfony.",
    packages: ["php", "nginx", "mariadb"],
  },
  { id: "node", label: "Node.js", description: "Node.js LTS with npm.", packages: ["node"] },
  { id: "python", label: "Python", description: "Python with pip and venv.", packages: ["python"] },
  { id: "custom", label: "Choose myself", description: "Pick exactly what to download.", packages: [] },
];

const SERVICE_PACKAGES = new Set(["nginx", "apache", "mariadb", "postgresql", "mongodb", "mailpit"]);

/** Default version: the newest the catalog lists that can be installed on this PC. */
function defaultVersion(p: PackageView): string | undefined {
  return p.versions.find((v) => !v.unavailableReason)?.version;
}

const panel = "mx-auto flex w-full max-w-3xl flex-col gap-4 p-6";

/**
 * First-run setup (app_flow.md §2, ADR 0014): welcome → where to keep things → what to
 * download → review → one setup operation. Skippable at every step; nothing needs admin.
 */
export function SetupWizard({ status, onDone }: { status: SetupStatus; onDone: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [error, setError] = useState<AppError | null>(null);
  const [selected, setSelected] = useState<Record<string, string>>({});
  const [operationId, setOperationId] = useState<string | null>(null);
  const catalog = useAsync(ipc.listPackages);

  const skip = async () => {
    try {
      await ipc.completeSetup();
      onDone();
    } catch (e) {
      setError(toAppError(e));
    }
  };

  return (
    <main className="min-h-screen bg-surface text-fg">
      <div className={panel}>
        <header className="flex items-center justify-between">
          <h1 className="text-xl font-semibold">Set up Devluma</h1>
          {step !== "running" && <Button onClick={() => void skip()}>Skip setup</Button>}
        </header>
        <ol aria-label="Setup steps" className="flex gap-2 text-xs text-fg-muted">
          {(["welcome", "storage", "stack", "review", "running"] as Step[]).map((s, i) => (
            <li
              key={s}
              aria-current={s === step ? "step" : undefined}
              className={s === step ? "font-semibold text-fg" : ""}
            >
              {i + 1}.{" "}
              {
                {
                  welcome: "Welcome",
                  storage: "Folder",
                  stack: "Tools",
                  review: "Review",
                  running: "Set up",
                }[s]
              }
            </li>
          ))}
        </ol>
        {error && <ErrorPanel error={error} />}
        {step === "welcome" && <Welcome status={status} onNext={() => setStep("storage")} />}
        {step === "storage" && (
          <Storage status={status} onBack={() => setStep("welcome")} onNext={() => setStep("stack")} />
        )}
        {step === "stack" &&
          (catalog.status === "success" ? (
            <Stack
              packages={catalog.data.packages}
              selected={selected}
              setSelected={setSelected}
              freeBytes={status.freeBytes}
              onBack={() => setStep("storage")}
              onNext={() => setStep("review")}
            />
          ) : catalog.status === "error" ? (
            <ErrorPanel error={catalog.error} onRetry={catalog.reload} />
          ) : (
            <p role="status">Loading the package catalog…</p>
          ))}
        {step === "review" && catalog.status === "success" && (
          <Review
            packages={catalog.data.packages}
            selected={selected}
            onBack={() => setStep("stack")}
            onStart={async () => {
              setError(null);
              try {
                const r = await ipc.runSetup({
                  items: Object.entries(selected).map(([packageId, version]) => ({ packageId, version })),
                });
                setOperationId(r.operationId);
                setStep("running");
              } catch (e) {
                setError(toAppError(e));
              }
            }}
          />
        )}
        {step === "running" && operationId && (
          <Running operationId={operationId} onFinish={() => void skip()} />
        )}
      </div>
    </main>
  );
}

function Welcome({ status, onNext }: { status: SetupStatus; onNext: () => void }) {
  return (
    <section aria-labelledby="welcome-heading" className="flex flex-col gap-3 text-sm">
      <h2 id="welcome-heading" className="text-lg font-semibold">
        Welcome
      </h2>
      <p>
        Devluma itself is small. PHP, web servers, databases and other tools are downloaded only when you
        choose them, from their official sources, and checked before they&apos;re used.
      </p>
      <ul className="list-inside list-disc space-y-1 text-fg-muted">
        <li>
          Nothing here needs administrator rights. Windows only asks later, when you add a local domain.
        </li>
        <li>Everything Devluma downloads stays in one folder, which you choose next.</li>
        <li>You can skip this and install things later from Runtimes &amp; Packages.</li>
      </ul>
      {status.missingPrerequisite && (
        <p className="rounded-md border border-warning p-3 text-warning">
          {status.missingPrerequisite} isn&apos;t installed. PHP and Apache need it; install it from Microsoft
          before setting them up. Everything else works without it.
        </p>
      )}
      {status.catalogProblems > 0 && (
        <p className="text-warning">The package catalog has problems; some packages may be unavailable.</p>
      )}
      <div>
        <Button variant="primary" onClick={onNext}>
          Next
        </Button>
      </div>
    </section>
  );
}

function Storage({
  status,
  onBack,
  onNext,
}: {
  status: SetupStatus;
  onBack: () => void;
  onNext: () => void;
}) {
  const exeDir = status.storage.exeDir ?? "";
  const [mode, setMode] = useState<"portable" | "other">(
    status.storage.mode === "custom" ? "other" : "portable",
  );
  const [folder, setFolder] = useState(status.storage.mode === "custom" ? status.storage.root : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [restart, setRestart] = useState(false);

  const next = async () => {
    if (!status.canChangeStorage) return onNext();
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.chooseStorage(mode === "portable" ? null : folder);
      if (r.restartNeeded) setRestart(true);
      else onNext();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="storage-heading" className="flex flex-col gap-3 text-sm">
      <h2 id="storage-heading" className="text-lg font-semibold">
        Where should Devluma keep everything?
      </h2>
      <p className="text-fg-muted">
        Downloaded tools (<span className="font-mono">bin</span>), databases, settings, logs, backups and
        temporary files all go in this one folder.
      </p>
      {status.canChangeStorage ? (
        <fieldset className="flex flex-col gap-2">
          <legend className="sr-only">Storage folder</legend>
          <div>
            <label className="flex items-center gap-2 font-medium">
              <input
                type="radio"
                name="storage"
                aria-describedby="storage-portable-desc"
                checked={mode === "portable"}
                onChange={() => setMode("portable")}
              />
              Devluma&apos;s own folder (portable)
            </label>
            <p id="storage-portable-desc" className="ml-6 text-xs text-fg-muted">
              <span className="block font-mono break-all">{exeDir}</span>
              Move or copy the whole folder and everything comes along.
            </p>
          </div>
          <div className="flex flex-col gap-1">
            <label className="flex items-center gap-2 font-medium">
              <input
                type="radio"
                name="storage"
                checked={mode === "other"}
                onChange={() => setMode("other")}
              />
              Another folder
            </label>
            <input
              aria-label="Folder path"
              value={folder}
              onChange={(e) => setFolder(e.target.value)}
              onFocus={() => setMode("other")}
              placeholder={String.raw`D:\DevlumaData`}
              className="ml-6 rounded-md border border-border bg-surface px-2 py-1.5 font-mono"
            />
          </div>
        </fieldset>
      ) : (
        <p>
          Devluma uses <span className="font-mono break-all">{status.storage.root}</span>
          {status.storage.exeDirWritable
            ? " (development build)."
            : " because its own folder can't be written to."}
        </p>
      )}
      {status.freeBytes !== null && (
        <p className="text-fg-muted">Free space: {formatBytes(status.freeBytes)}</p>
      )}
      {error && <ErrorPanel error={error} />}
      {restart ? (
        <div role="status" className="flex flex-col gap-2 rounded-md border border-border p-3">
          <p>
            Devluma needs to restart to use the new folder. Nothing already in the current folder is moved; to
            keep it, close Devluma and move it yourself.
          </p>
          <div>
            <Button variant="primary" onClick={() => void ipc.restartApp()}>
              Restart Devluma
            </Button>
          </div>
        </div>
      ) : (
        <div className="flex gap-2">
          <Button onClick={onBack}>Back</Button>
          <Button
            variant="primary"
            pending={busy}
            disabled={mode === "other" && !folder.trim()}
            onClick={() => void next()}
          >
            Next
          </Button>
        </div>
      )}
    </section>
  );
}

function Stack({
  packages,
  selected,
  setSelected,
  freeBytes,
  onBack,
  onNext,
}: {
  packages: PackageView[];
  selected: Record<string, string>;
  setSelected: (s: Record<string, string>) => void;
  freeBytes: number | null;
  onBack: () => void;
  onNext: () => void;
}) {
  const [preset, setPreset] = useState<string | null>(null);
  const byId = new Map(packages.map((p) => [p.id, p]));
  const installed = (p: PackageView) => p.versions.some((v) => v.installation?.status === "installed");

  const applyPreset = (id: string) => {
    setPreset(id);
    const next: Record<string, string> = {};
    for (const pid of PRESETS.find((p) => p.id === id)?.packages ?? []) {
      const p = byId.get(pid);
      const v = p && defaultVersion(p);
      if (p && v && !installed(p)) next[pid] = v;
    }
    setSelected(next);
  };

  const toggle = (p: PackageView, on: boolean, pickOneOf?: string[]) => {
    const next = { ...selected };
    if (on) {
      for (const other of pickOneOf ?? []) delete next[other];
      const v = defaultVersion(p);
      if (v) next[p.id] = v;
    } else {
      delete next[p.id];
    }
    setSelected(next);
  };

  const total = Object.entries(selected).reduce((sum, [pid, ver]) => {
    const v = byId.get(pid)?.versions.find((x) => x.version === ver);
    return sum + (v?.sizeBytes ?? 0);
  }, 0);
  // Downloads are unpacked next to the archive before it's removed.
  const needed = total * 4;

  return (
    <section aria-labelledby="stack-heading" className="flex flex-col gap-4 text-sm">
      <h2 id="stack-heading" className="text-lg font-semibold">
        What do you want to download?
      </h2>
      <fieldset className="grid gap-2 sm:grid-cols-2">
        <legend className="mb-1 text-fg-muted">Start from a preset</legend>
        {PRESETS.map((p) => (
          <div key={p.id} className="rounded-md border border-border p-3">
            <label className="flex items-center gap-2 font-medium">
              <input
                type="radio"
                name="preset"
                aria-describedby={`preset-${p.id}-desc`}
                checked={preset === p.id}
                onChange={() => applyPreset(p.id)}
              />
              {p.label}
            </label>
            <p id={`preset-${p.id}-desc`} className="ml-6 text-xs text-fg-muted">
              {p.description}
            </p>
          </div>
        ))}
      </fieldset>

      {GROUPS.map((g) => (
        <fieldset key={g.title} className="flex flex-col gap-1">
          <legend className="font-medium">{g.title}</legend>
          {g.ids
            .map((id) => byId.get(id))
            .filter((p): p is PackageView => Boolean(p))
            .map((p) => {
              const ver = selected[p.id];
              const already = installed(p);
              return (
                <div key={p.id} className="flex flex-wrap items-center gap-3">
                  <label className="flex min-w-48 items-center gap-2">
                    <input
                      type="checkbox"
                      disabled={already || !defaultVersion(p)}
                      checked={Boolean(ver)}
                      onChange={(e) => {
                        setPreset("custom");
                        toggle(p, e.target.checked, g.pickOne ? g.ids.filter((x) => x !== p.id) : undefined);
                      }}
                    />
                    {p.displayName}
                  </label>
                  {already ? (
                    <span className="text-xs text-fg-muted">Already installed</span>
                  ) : ver ? (
                    <select
                      aria-label={`${p.displayName} version`}
                      value={ver}
                      onChange={(e) => setSelected({ ...selected, [p.id]: e.target.value })}
                      className="rounded-md border border-border bg-surface px-2 py-1"
                    >
                      {p.versions
                        .filter((v) => !v.unavailableReason)
                        .map((v) => (
                          <option key={v.key} value={v.version}>
                            {v.version} — {formatBytes(v.sizeBytes)}
                          </option>
                        ))}
                    </select>
                  ) : null}
                  {SERVICE_PACKAGES.has(p.id) && ver && (
                    <span className="text-xs text-fg-muted">A service is created on a free port.</span>
                  )}
                </div>
              );
            })}
        </fieldset>
      ))}

      <p aria-live="polite">
        Download: <span className="font-medium">{formatBytes(total)}</span>
        {total > 0 && (
          <span className="text-fg-muted"> · needs about {formatBytes(needed)} free while installing</span>
        )}
        {freeBytes !== null && needed > freeBytes && (
          <span className="block text-danger">Not enough free space in the chosen folder.</span>
        )}
      </p>
      <div className="flex gap-2">
        <Button onClick={onBack}>Back</Button>
        <Button
          variant="primary"
          disabled={total === 0 || (freeBytes !== null && needed > freeBytes)}
          onClick={onNext}
        >
          Review
        </Button>
      </div>
    </section>
  );
}

function Review({
  packages,
  selected,
  onBack,
  onStart,
}: {
  packages: PackageView[];
  selected: Record<string, string>;
  onBack: () => void;
  onStart: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const rows = Object.entries(selected).flatMap(([pid, ver]) => {
    const p = packages.find((x) => x.id === pid);
    const v = p?.versions.find((x) => x.version === ver);
    return p && v ? [{ p, v }] : [];
  });
  return (
    <section aria-labelledby="review-heading" className="flex flex-col gap-3 text-sm">
      <h2 id="review-heading" className="text-lg font-semibold">
        Review
      </h2>
      <table className="w-full text-left">
        <thead className="text-fg-muted">
          <tr>
            {["Package", "Version", "From", "License", "Size"].map((h) => (
              <th key={h} scope="col" className="py-1 pr-3 font-medium">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ p, v }) => (
            <tr key={v.key}>
              <td className="py-1 pr-3">{p.displayName}</td>
              <td className="py-1 pr-3 font-mono">{v.version}</td>
              <td className="py-1 pr-3 font-mono text-xs">{v.sourceHost}</td>
              <td className="py-1 pr-3">{p.license.id}</td>
              <td className="py-1 pr-3">{formatBytes(v.sizeBytes)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-fg-muted">
        Each download is checked against its published SHA-256 before it&apos;s unpacked. Runtimes become the
        default version if you don&apos;t have one yet; web servers, databases and Mailpit get a service on a
        free port (databases get a new data folder; the root/admin user has no password and only this computer
        can connect).
      </p>
      <div className="flex gap-2">
        <Button onClick={onBack} disabled={busy}>
          Back
        </Button>
        <Button
          variant="primary"
          pending={busy}
          onClick={() => {
            setBusy(true);
            void onStart().finally(() => setBusy(false));
          }}
        >
          Download and set up
        </Button>
      </div>
    </section>
  );
}

function Running({ operationId, onFinish }: { operationId: string; onFinish: () => void }) {
  const { operations } = useOperations();
  const op = operations.find((o) => o.operationId === operationId);
  const finished = op && ["succeeded", "failed", "cancelled"].includes(op.status);
  return (
    <section aria-labelledby="running-heading" className="flex flex-col gap-3 text-sm">
      <h2 id="running-heading" className="text-lg font-semibold">
        {finished ? (op.status === "succeeded" ? "All set" : "Set up with some problems") : "Setting up…"}
      </h2>
      {!finished && (
        <div role="status" className="flex flex-col gap-2">
          <span>{op?.phase ?? "Starting…"}</span>
          {op?.current !== undefined && op.total !== undefined && (
            <progress max={op.total} value={op.current} aria-label="Setup progress" className="w-full" />
          )}
          <span className="text-fg-muted">
            You can keep this window open; downloads continue in the background.
          </span>
        </div>
      )}
      {finished && op.status !== "succeeded" && <p className="text-warning">{op.safeMessage}</p>}
      {finished && (
        <>
          <ul className="list-inside list-disc space-y-1">
            <li>
              Add a project folder on the Projects page, then give it a local domain on Sites &amp; Domains.
            </li>
            <li>Database connection details are on the Databases page.</li>
            <li>Start or stop services on the Services page.</li>
          </ul>
          <div>
            <Button variant="primary" onClick={onFinish}>
              Open Devluma
            </Button>
          </div>
        </>
      )}
    </section>
  );
}
