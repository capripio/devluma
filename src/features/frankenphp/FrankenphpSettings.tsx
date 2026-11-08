import { useState, type FormEvent } from "react";
import { Link } from "react-router";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { FrankenphpStatus } from "../../types/dto";

const input = "rounded-md border border-border bg-surface px-2 py-1.5";

/** Settings → FrankenPHP: which frankenphp.exe to use (ADR 0019). Never downloaded by Devluma. */
export function FrankenphpSettings() {
  const status = useAsync(ipc.getFrankenphpStatus);
  return (
    <section
      aria-labelledby="frankenphp-heading"
      className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm"
    >
      <h2 id="frankenphp-heading" className="font-semibold">
        FrankenPHP
      </h2>
      <p className="text-pretty text-fg-muted">
        FrankenPHP is a web server with PHP built in. Devluma doesn&apos;t download it: FrankenPHP rebuilds
        its Windows release every day, so there&apos;s no fixed file Devluma could verify. Download the
        Windows zip from FrankenPHP&apos;s GitHub releases yourself, unzip it, and choose its frankenphp.exe
        here. Devluma checks that it runs and never changes that folder.
      </p>
      {status.status === "pending" && <p role="status">Checking FrankenPHP…</p>}
      {status.status === "error" && <ErrorPanel error={status.error} onRetry={status.reload} />}
      {status.status === "success" && <FrankenphpForm status={status.data} onChanged={status.refresh} />}
    </section>
  );
}

function FrankenphpForm({ status, onChanged }: { status: FrankenphpStatus; onChanged: () => void }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState<"path" | "remove" | null>(null);
  const [error, setError] = useState<AppError | null>(null);

  const run = async (kind: "path" | "remove", action: () => Promise<unknown>) => {
    setBusy(kind);
    setError(null);
    try {
      await action();
      if (kind === "path") setPath("");
      onChanged();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(null);
    }
  };

  const choosePath = (e: FormEvent) => {
    e.preventDefault();
    void run("path", () => ipc.setFrankenphpPath(path.trim()));
  };

  return (
    <>
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
        <dt className="text-fg-muted">frankenphp.exe</dt>
        <dd className="font-mono break-all">{status.path ?? "Not chosen"}</dd>
        {status.version && (
          <>
            <dt className="text-fg-muted">Versions</dt>
            <dd>
              FrankenPHP {status.version} with PHP {status.phpVersion}
            </dd>
          </>
        )}
        {status.path && (
          <>
            <dt className="text-fg-muted">Used by</dt>
            <dd>{status.services.length > 0 ? status.services.join(", ") : "No services yet"}</dd>
          </>
        )}
      </dl>
      {status.problem && <p className="text-warning">{status.problem}</p>}
      {status.installationId && !status.problem && status.services.length === 0 && (
        <p>
          Create a FrankenPHP service on the{" "}
          <Link to="/services" className="text-accent hover:underline">
            Services
          </Link>{" "}
          page, then use it for sites. Sites served by FrankenPHP use its built-in PHP, not the project&apos;s
          PHP version.
        </p>
      )}

      {status.detected.length > 0 && (
        <div className="flex flex-col gap-1">
          <span className="text-fg-muted">Found on this computer:</span>
          <ul className="flex flex-col gap-1">
            {status.detected.map((d) => (
              <li key={d} className="flex flex-wrap items-center gap-2">
                <span className="font-mono break-all">{d}</span>
                {d !== status.path && (
                  <Button
                    pending={busy === "path"}
                    disabled={busy !== null}
                    onClick={() => void run("path", () => ipc.setFrankenphpPath(d))}
                    aria-label={`Use ${d}`}
                  >
                    Use this
                  </Button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      <form
        onSubmit={choosePath}
        aria-label="Choose frankenphp.exe"
        className="flex flex-wrap items-end gap-2"
      >
        <label className="flex min-w-72 flex-1 flex-col gap-1">
          <span className="text-fg-muted">Path to frankenphp.exe</span>
          <input
            required
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder={String.raw`C:\Tools\frankenphp\frankenphp.exe`}
            className={`${input} font-mono`}
          />
        </label>
        <Button type="submit" variant="primary" pending={busy === "path"} disabled={busy !== null}>
          Use this frankenphp.exe
        </Button>
        {status.path && (
          <Button
            pending={busy === "remove"}
            disabled={busy !== null || status.services.length > 0}
            onClick={() => void run("remove", ipc.removeFrankenphp)}
          >
            Forget FrankenPHP
          </Button>
        )}
      </form>
      {status.path && status.services.length > 0 && (
        <p className="text-xs text-fg-muted">Remove its services first to forget it.</p>
      )}
      {error && <ErrorPanel error={error} />}
    </>
  );
}
