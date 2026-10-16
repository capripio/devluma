import { useState, type FormEvent } from "react";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { ServiceView } from "../../types/dto";

const input = "w-28 rounded-md border border-border bg-surface px-2 py-1.5 font-mono";

/** Changes a stopped service's port (and UI port, if it has one). */
export function PortsForm({ service: s, disabled }: { service: ServiceView; disabled: boolean }) {
  const [port, setPort] = useState(String(s.port ?? ""));
  const [adminPort, setAdminPort] = useState(s.adminPort === null ? "" : String(s.adminPort));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [saved, setSaved] = useState(false);
  const changed =
    port !== String(s.port ?? "") || (s.adminPort !== null && adminPort !== String(s.adminPort));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      await ipc.setServicePorts(s.id, Number(port), s.adminPort === null ? null : Number(adminPort));
      setSaved(true);
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      aria-label="Ports"
      className="flex flex-col gap-2 rounded-md border border-border p-4"
    >
      <h2 className="font-semibold">Ports</h2>
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Port</span>
          <input
            type="number"
            min={1}
            max={65535}
            required
            disabled={disabled}
            value={port}
            onChange={(e) => setPort(e.target.value)}
            className={input}
          />
        </label>
        {s.adminPort !== null && (
          <label className="flex flex-col gap-1">
            <span className="text-fg-muted">Web interface port</span>
            <input
              type="number"
              min={1}
              max={65535}
              required
              disabled={disabled}
              value={adminPort}
              onChange={(e) => setAdminPort(e.target.value)}
              className={input}
            />
          </label>
        )}
        <Button type="submit" pending={busy} disabled={disabled || !changed}>
          Save ports
        </Button>
      </div>
      {disabled && <p className="text-xs text-fg-muted">Stop the service to change its ports.</p>}
      {saved && (
        <p role="status" className="text-xs text-fg-muted">
          Saved. The service uses the new ports from its next start.
        </p>
      )}
      {error && <ErrorPanel error={error} />}
    </form>
  );
}
