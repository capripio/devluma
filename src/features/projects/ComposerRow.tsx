import { useState } from "react";
import { Link } from "react-router";
import { useOnOperationsSettled } from "../../app/operations";
import { Button } from "../../components/Button";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ProjectView } from "../../types/dto";

const COMPOSER_OPERATIONS = new Set([
  "composer_install",
  "composer_update",
  "edit_config",
  "install_package",
  "switch_runtime",
]);

/** Composer for one project: its version, missing PHP extensions, install / update (ADR 0020). */
export function ComposerRow({ project }: { project: ProjectView }) {
  const status = useAsync(() => ipc.getComposerStatus(project.id));
  useOnOperationsSettled((op) => COMPOSER_OPERATIONS.has(op.operationType), status.refresh);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const act = (pending: string, run: () => Promise<unknown>, done: string) => {
    setBusy(true);
    setNote(pending);
    run().then(
      () => {
        setNote(done);
        setBusy(false);
        status.refresh();
      },
      (e: unknown) => {
        setNote(toAppError(e).message);
        setBusy(false);
      },
    );
  };

  if (status.status !== "success") {
    return (
      <>
        <dt className="text-fg-muted">Composer</dt>
        <dd className="text-fg-muted">{status.status === "error" ? status.error.message : "Checking…"}</dd>
      </>
    );
  }
  const s = status.data;
  return (
    <>
      <dt className="text-fg-muted">Composer</dt>
      <dd className="flex flex-col items-start gap-2">
        {s.composerVersion ? (
          <span>
            <span className="font-mono text-xs">{s.composerVersion}</span>
            {s.phpVersion && <span className="text-fg-muted"> with PHP {s.phpVersion}</span>}
            <span className="text-fg-muted"> · works in this project&apos;s terminal</span>
          </span>
        ) : (
          <span className="text-fg-muted">
            Not installed. Install it on{" "}
            <Link to="/runtimes" className="text-accent hover:underline">
              Runtimes &amp; Packages
            </Link>
            .
          </span>
        )}
        {s.composerVersion && s.missingExtensions.length > 0 && (
          <span className="flex flex-wrap items-center gap-2 text-warning">
            PHP {s.phpVersion} doesn&apos;t load {s.missingExtensions.join(", ")}, which Composer needs.
            <Button
              disabled={busy}
              onClick={() =>
                act(
                  "Turning them on…",
                  () => ipc.enableComposerExtensions(project.id),
                  `Turned on ${s.missingExtensions.join(", ")} in php.ini (a backup was taken).`,
                )
              }
            >
              Turn them on
            </Button>
          </span>
        )}
        {s.composerVersion && s.hasComposerJson && (
          <span className="flex flex-wrap gap-2">
            <Button
              disabled={busy || s.running || s.missingExtensions.length > 0}
              onClick={() =>
                act("Starting…", () => ipc.runComposer(project.id, "install"), "composer install is running.")
              }
            >
              composer install
            </Button>
            <Button
              disabled={busy || s.running || s.missingExtensions.length > 0}
              onClick={() =>
                act("Starting…", () => ipc.runComposer(project.id, "update"), "composer update is running.")
              }
            >
              composer update
            </Button>
          </span>
        )}
        {s.running && <span className="text-fg-muted">Composer is running for this project…</span>}
        {note && (
          <span aria-live="polite" className="text-fg-muted">
            {note}
          </span>
        )}
      </dd>
    </>
  );
}
