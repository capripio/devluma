import { useState } from "react";
import { Button } from "../../components/Button";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { NodePackageManager, PackageView } from "../../types/dto";

const MANAGERS: { id: NodePackageManager; label: string }[] = [
  { id: "pnpm", label: "pnpm" },
  { id: "yarn", label: "Yarn" },
];

/** pnpm and Yarn per installed Node.js version, through its Corepack (ADR 0020). */
export function NodePackageManagers({ pkg }: { pkg: PackageView }) {
  const installed = pkg.versions.filter((v) => v.installation?.status === "installed");
  if (installed.length === 0) return null;
  return (
    <section aria-labelledby="node-pm-heading" className="flex flex-col gap-2 text-sm">
      <h2 id="node-pm-heading" className="text-sm font-semibold">
        Package managers
      </h2>
      <p className="text-fg-muted">
        npm comes with Node.js. pnpm and Yarn are added through Node.js&apos;s Corepack: the command goes into
        that Node.js folder (so it&apos;s on PATH wherever that version is), and the tool downloads from npm
        the first time you run it.
      </p>
      {installed.map((v) => (
        <VersionManagers key={v.key} installationId={v.installation!.id} version={v.version} />
      ))}
    </section>
  );
}

function VersionManagers({ installationId, version }: { installationId: string; version: string }) {
  const status = useAsync(() => ipc.getNodeTools(installationId));
  const [chosen, setChosen] = useState<NodePackageManager[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  if (status.status !== "success") {
    return (
      <p className="text-fg-muted">
        Node.js {version}: {status.status === "error" ? status.error.message : "checking…"}
      </p>
    );
  }
  const tools = status.data;
  if (!tools.corepack) {
    return (
      <p className="text-fg-muted">
        Node.js {version} doesn&apos;t include Corepack, so pnpm and Yarn can&apos;t be added here. Install
        them with npm instead (<span className="font-mono text-xs">npm install -g pnpm</span>).
      </p>
    );
  }
  const value = chosen ?? tools.enabled;
  const changed = chosen !== null && [...chosen].sort().join() !== [...tools.enabled].sort().join();
  const save = () => {
    setPending(true);
    setNote(null);
    ipc.setNodeTools(installationId, value).then(
      () => {
        setChosen(null);
        setPending(false);
        setNote("Saved.");
        status.refresh();
      },
      (e: unknown) => {
        setPending(false);
        setNote(toAppError(e).message);
      },
    );
  };
  return (
    <fieldset className="flex flex-wrap items-center gap-3">
      <legend className="sr-only">Package managers for Node.js {version}</legend>
      <span className="w-32 font-mono text-xs">Node.js {version}</span>
      {MANAGERS.map((m) => (
        <label key={m.id} className="flex items-center gap-1.5">
          <input
            type="checkbox"
            checked={value.includes(m.id)}
            onChange={(e) => setChosen(e.target.checked ? [...value, m.id] : value.filter((x) => x !== m.id))}
          />
          {m.label}
        </label>
      ))}
      <Button onClick={save} disabled={!changed} pending={pending}>
        Save
      </Button>
      {note && (
        <span aria-live="polite" className="text-fg-muted">
          {note}
        </span>
      )}
    </fieldset>
  );
}
