import { useState, type FormEvent } from "react";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { NgrokStatus } from "../../types/dto";

const input = "rounded-md border border-border bg-surface px-2 py-1.5";

/** Settings → Sharing (ngrok): which ngrok.exe to use and the authtoken (ADR 0011). */
export function NgrokSettings() {
  const status = useAsync(ipc.getNgrokStatus);
  return (
    <section
      aria-labelledby="ngrok-heading"
      className="flex flex-col gap-3 rounded-lg border border-border p-4 text-sm"
    >
      <h2 id="ngrok-heading" className="font-semibold">
        Sharing with ngrok
      </h2>
      <p className="text-fg-muted">
        Devluma doesn&apos;t download ngrok. Install it yourself from ngrok.com, then choose its ngrok.exe
        here. Your authtoken is kept in Windows Credential Manager and never shown again or written to
        Devluma&apos;s files.
      </p>
      {status.status === "pending" && <p role="status">Checking ngrok…</p>}
      {status.status === "error" && <ErrorPanel error={status.error} onRetry={status.reload} />}
      {status.status === "success" && <NgrokForm status={status.data} onChanged={status.refresh} />}
    </section>
  );
}

function NgrokForm({ status, onChanged }: { status: NgrokStatus; onChanged: () => void }) {
  const [path, setPath] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState<"path" | "token" | "remove" | null>(null);
  const [error, setError] = useState<AppError | null>(null);

  const run = async (kind: "path" | "token" | "remove", action: () => Promise<unknown>) => {
    setBusy(kind);
    setError(null);
    try {
      await action();
      if (kind === "token") setToken("");
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
    void run("path", () => ipc.setNgrokPath(path.trim()));
  };
  const saveToken = (e: FormEvent) => {
    e.preventDefault();
    void run("token", () => ipc.setNgrokToken(token));
  };

  return (
    <>
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
        <dt className="text-fg-muted">ngrok.exe</dt>
        <dd className="font-mono break-all">
          {status.path ?? "Not chosen"}
          {status.version && <span className="ml-2 font-sans text-fg-muted">(ngrok {status.version})</span>}
        </dd>
        <dt className="text-fg-muted">Authtoken</dt>
        <dd>{status.tokenSaved ? "Saved in Windows Credential Manager" : "Not saved"}</dd>
      </dl>
      {status.problem && <p className="text-warning">{status.problem}</p>}

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
                    onClick={() => void run("path", () => ipc.setNgrokPath(d))}
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

      <form onSubmit={choosePath} aria-label="Choose ngrok.exe" className="flex flex-wrap items-end gap-2">
        <label className="flex min-w-72 flex-1 flex-col gap-1">
          <span className="text-fg-muted">Path to ngrok.exe</span>
          <input
            required
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="C:\\Tools\\ngrok\\ngrok.exe"
            className={`${input} font-mono`}
          />
        </label>
        <Button type="submit" pending={busy === "path"} disabled={busy !== null}>
          Use this ngrok.exe
        </Button>
      </form>

      <form onSubmit={saveToken} aria-label="ngrok authtoken" className="flex flex-wrap items-end gap-2">
        <label className="flex min-w-72 flex-1 flex-col gap-1">
          <span className="text-fg-muted">{status.tokenSaved ? "Replace authtoken" : "Authtoken"}</span>
          <input
            required
            type="password"
            autoComplete="off"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            className={`${input} font-mono`}
          />
        </label>
        <Button type="submit" variant="primary" pending={busy === "token"} disabled={busy !== null}>
          Save to Credential Manager
        </Button>
        {status.tokenSaved && (
          <Button
            pending={busy === "remove"}
            disabled={busy !== null}
            onClick={() => void run("remove", ipc.removeNgrokToken)}
          >
            Remove authtoken
          </Button>
        )}
      </form>
      {error && <ErrorPanel error={error} />}
    </>
  );
}
