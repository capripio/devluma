import { useState, type FormEvent } from "react";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { AppUpdate, UpdateSettings as Settings } from "../../types/dto";
import { LoadingState } from "../../components/LoadingState";

const input = "rounded-md border border-border bg-surface px-2 py-1.5 font-mono";

/** Settings → Updates (ADR 0013): off until URLs are set; everything is signature-checked. */
export function UpdateSettings() {
  const settings = useAsync(ipc.getUpdateSettings);
  return (
    <section
      aria-labelledby="updates-heading"
      className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm"
    >
      <h2 id="updates-heading" className="font-semibold">
        Updates
      </h2>
      {settings.status === "pending" && <LoadingState label="Loading…" shape="inline" />}
      {settings.status === "error" && <ErrorPanel error={settings.error} onRetry={settings.reload} />}
      {settings.status === "success" && <UpdateForm settings={settings.data} onChanged={settings.refresh} />}
    </section>
  );
}

function UpdateForm({ settings: s, onChanged }: { settings: Settings; onChanged: () => void }) {
  const [appUrl, setAppUrl] = useState(s.appUrl ?? "");
  const [catalogUrl, setCatalogUrl] = useState(s.catalogUrl ?? "");
  const [busy, setBusy] = useState<"save" | "app" | "catalog" | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  const [installing, setInstalling] = useState(false);

  const run = async (kind: "save" | "app" | "catalog", action: () => Promise<void>) => {
    setBusy(kind);
    setError(null);
    setMessage(null);
    try {
      await action();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(null);
    }
  };

  const save = (e: FormEvent) => {
    e.preventDefault();
    void run("save", async () => {
      await ipc.setUpdateUrls(appUrl, catalogUrl);
      setMessage("Saved.");
      onChanged();
    });
  };

  return (
    <>
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
        <dt className="text-fg-muted">Devluma</dt>
        <dd>{s.currentVersion}</dd>
        <dt className="text-fg-muted">Package catalog</dt>
        <dd>
          {s.catalogVersion}
          {s.pendingCatalogVersion && (
            <span className="ml-2 text-fg-muted">
              ({s.pendingCatalogVersion} verified — used after a restart)
            </span>
          )}
        </dd>
      </dl>
      <p className="text-fg-muted">
        Updates are off until an update address is set. Devluma only installs app updates and package catalogs
        that are signed with its own keys; anything else is rejected.
      </p>
      <form onSubmit={save} aria-label="Update addresses" className="flex flex-col gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">App update address (https)</span>
          <input value={appUrl} onChange={(e) => setAppUrl(e.target.value)} className={input} />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Package catalog address (https, catalog.json)</span>
          <input value={catalogUrl} onChange={(e) => setCatalogUrl(e.target.value)} className={input} />
        </label>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" pending={busy === "save"} disabled={busy !== null}>
            Save addresses
          </Button>
          <Button
            pending={busy === "app"}
            disabled={busy !== null || !s.appUrl}
            onClick={() =>
              void run("app", async () => {
                const u = await ipc.checkAppUpdate();
                setUpdate(u.availableVersion ? u : null);
                if (!u.availableVersion) setMessage(`Devluma ${u.currentVersion} is up to date.`);
              })
            }
          >
            Check for app update
          </Button>
          <Button
            pending={busy === "catalog"}
            disabled={busy !== null || !s.catalogUrl}
            onClick={() =>
              void run("catalog", async () => {
                const r = await ipc.checkCatalogUpdate();
                setMessage(r.message);
                onChanged();
              })
            }
          >
            Check for catalog update
          </Button>
        </div>
      </form>
      {message && (
        <p role="status" className="text-fg-muted">
          {message}
        </p>
      )}
      {error && <ErrorPanel error={error} />}
      {update && (
        <div className="flex flex-col gap-2 rounded-md border border-border p-3">
          <p>
            Devluma <span className="font-medium">{update.availableVersion}</span> is available.
          </p>
          {update.notes && <p className="whitespace-pre-wrap text-fg-muted">{update.notes}</p>}
          <div>
            <Button variant="primary" onClick={() => setInstalling(true)}>
              Install and restart…
            </Button>
          </div>
        </div>
      )}
      {installing && update && (
        <ConfirmChangeDialog
          open
          onOpenChange={(o) => !o && setInstalling(false)}
          title={`Install Devluma ${update.availableVersion}?`}
          target={s.appUrl ?? ""}
          impact={
            <ul className="list-inside list-disc space-y-1">
              <li>The update is downloaded and its signature checked; an unsigned update is refused.</li>
              <li>Running services and shared sites are stopped, then Devluma restarts.</li>
              <li>Your projects, data, settings and backups are kept.</li>
            </ul>
          }
          rollback="Install the previous version's installer to go back; your data folder is unaffected."
          confirmLabel="Install and restart"
          onConfirm={async () => {
            await ipc.installAppUpdate();
          }}
        />
      )}
    </>
  );
}
