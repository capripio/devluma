import { useState, type FormEvent, type ReactNode } from "react";
import { Link } from "react-router";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type {
  DefaultPorts,
  LaunchAtLogin,
  Preferences,
  PreferencesView,
  TerminalProgram,
} from "../../types/dto";
import { OpenTerminalButton } from "../terminal/OpenTerminalButton";
import { TERMINAL_LABELS } from "../terminal/terminal";

const input = "rounded-md border border-border bg-surface px-2 py-1.5";

export interface PreferencesProps {
  view: PreferencesView;
  /** Stores `patch` on top of the current preferences; rejects with the backend's error. */
  save: (patch: Partial<Preferences>) => Promise<void>;
}

export function SettingsSection({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section aria-labelledby={id} className="flex flex-col gap-3 rounded-lg border border-border p-4 text-sm">
      <h2 id={id} className="font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}

/** Save button + "Saved." / error line shared by the preference forms. */
function useSaver(save: PreferencesProps["save"]) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [saved, setSaved] = useState(false);
  const run = (patch: Partial<Preferences>) => (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setSaved(false);
    save(patch)
      .then(
        () => setSaved(true),
        (err: unknown) => setError(toAppError(err)),
      )
      .finally(() => setBusy(false));
  };
  const footer = (
    <div className="flex flex-col items-start gap-2">
      <Button type="submit" disabled={busy}>
        {busy ? "Saving…" : "Save"}
      </Button>
      {saved && (
        <p role="status" className="text-fg-muted">
          Saved.
        </p>
      )}
      {error && <ErrorPanel error={error} />}
    </div>
  );
  return { run, footer, reset: () => setSaved(false) };
}

const TERMINALS: TerminalProgram[] = ["power_shell", "command_prompt", "cmder"];

/** Settings → Sites & terminal: the suffix for new site domains and the terminal program. */
export function SitesAndTerminalSettings({ view, save }: PreferencesProps) {
  const [suffix, setSuffix] = useState(view.preferences.domainSuffix);
  const [terminal, setTerminal] = useState(view.preferences.terminal);
  const { run, footer, reset } = useSaver(save);
  // What the backend will store (it also converts international names to IDNA).
  const typedSuffix = suffix
    .trim()
    .replace(/^\.+|\.+$/g, "")
    .toLowerCase();

  return (
    <SettingsSection id="sites-terminal-heading" title="Sites & terminal">
      <form onSubmit={run({ domainSuffix: suffix, terminal })} className="flex flex-col gap-4">
        <div className="flex flex-col gap-1">
          <label htmlFor="domain-suffix" className="text-fg-muted">
            Domain suffix for new sites
          </label>
          <div className="flex items-center gap-1">
            <span aria-hidden="true" className="font-mono text-fg-muted">
              .
            </span>
            <input
              id="domain-suffix"
              type="text"
              required
              spellCheck={false}
              autoComplete="off"
              list="domain-suffix-suggestions"
              placeholder="test"
              value={suffix}
              onChange={(e) => {
                setSuffix(e.target.value);
                reset();
              }}
              className={`${input} w-48 font-mono`}
            />
            <datalist id="domain-suffix-suggestions">
              {view.suggestedSuffixes.map((s) => (
                <option key={s} value={s} />
              ))}
            </datalist>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-fg-muted">
            <span>Suggestions:</span>
            {view.suggestedSuffixes.map((s) => (
              <button
                key={s}
                type="button"
                onClick={() => {
                  setSuffix(s);
                  reset();
                }}
                className="rounded border border-border px-1.5 font-mono hover:bg-surface-sunken"
              >
                .{s}
              </button>
            ))}
          </div>
          <p className="text-fg-muted">
            New sites are suggested as <span className="font-mono">my-app.{typedSuffix || "test"}</span>.
            Existing sites keep their domain. HTTPS certificates are available for suffixes ending in .test,
            .localhost, .internal, .example, .invalid or .local.
          </p>
          {typedSuffix === view.preferences.domainSuffix ? (
            view.suffixAdvice.warnings.map((w) => (
              <p key={w} className="text-warning">
                {w}
              </p>
            ))
          ) : (
            <p className="text-fg-muted">Save to check this suffix.</p>
          )}
        </div>

        <fieldset className="flex flex-col gap-1">
          <legend className="text-fg-muted">Terminal for &quot;Open terminal&quot;</legend>
          <div className="mt-1 flex flex-wrap gap-4">
            {TERMINALS.map((t) => (
              <label key={t} className="flex items-center gap-2">
                <input
                  type="radio"
                  name="terminal"
                  value={t}
                  checked={terminal === t}
                  onChange={() => {
                    setTerminal(t);
                    reset();
                  }}
                  className="accent-accent"
                />
                {TERMINAL_LABELS[t]}
              </label>
            ))}
          </div>
          {terminal === "cmder" &&
            (view.cmderInstalled ? (
              <p className="text-fg-muted">
                Cmder keeps its own settings in Devluma&apos;s <span className="font-mono">config\cmder</span>{" "}
                folder, so they survive Cmder updates.
              </p>
            ) : (
              <p className="text-warning">
                Cmder isn&apos;t installed yet.{" "}
                <Link to="/runtimes/cmder" className="text-accent hover:underline">
                  Install it from Runtimes &amp; Packages
                </Link>
                .
              </p>
            ))}
          <p className="text-fg-muted">
            Project terminals put the project&apos;s runtimes first on PATH for that window only; nothing
            changes system-wide.
          </p>
        </fieldset>
        {footer}
      </form>
      <OpenTerminalButton align="start" />
    </SettingsSection>
  );
}

const PORT_FIELDS: { key: keyof DefaultPorts; label: string }[] = [
  { key: "nginx", label: "Nginx" },
  { key: "apache", label: "Apache" },
  { key: "mariadb", label: "MariaDB" },
  { key: "postgresql", label: "PostgreSQL" },
  { key: "mongodb", label: "MongoDB" },
  { key: "mailpit", label: "Mailpit SMTP" },
  { key: "mailpitWeb", label: "Mailpit web interface" },
];

function toPort(value: string): number {
  const n = Number(value);
  return Number.isInteger(n) && n >= 0 && n <= 65535 ? n : 0;
}

/** Settings → Ports: defaults for new services, the HTTPS port, and existing services' ports. */
export function PortSettings({ view, save }: PreferencesProps) {
  const p = view.preferences;
  const [ports, setPorts] = useState<Record<keyof DefaultPorts, string>>(
    () =>
      Object.fromEntries(PORT_FIELDS.map((f) => [f.key, String(p.defaultPorts[f.key])])) as Record<
        keyof DefaultPorts,
        string
      >,
  );
  const [httpsAuto, setHttpsAuto] = useState(p.httpsPort === null);
  const [httpsPort, setHttpsPort] = useState(String(p.httpsPort ?? 8443));
  const { run, footer, reset } = useSaver(save);
  const services = useAsync(ipc.listServices);

  const patch: Partial<Preferences> = {
    defaultPorts: Object.fromEntries(PORT_FIELDS.map((f) => [f.key, toPort(ports[f.key])])) as DefaultPorts,
    httpsPort: httpsAuto ? null : toPort(httpsPort),
  };

  return (
    <SettingsSection id="ports-heading" title="Ports">
      <form onSubmit={run(patch)} className="flex flex-col gap-4">
        <fieldset>
          <legend className="text-fg-muted">Default ports for new services</legend>
          <p className="mt-1 text-fg-muted">
            Pre-filled when you add a service and used by the setup wizard (which picks the next free port if
            one is taken). Existing services keep their ports.
          </p>
          <div className="mt-2 grid grid-cols-[repeat(auto-fill,minmax(11rem,1fr))] gap-3">
            {PORT_FIELDS.map((f) => (
              <label key={f.key} className="flex flex-col gap-1">
                <span>{f.label}</span>
                <input
                  type="number"
                  min={1}
                  max={65535}
                  required
                  value={ports[f.key]}
                  onChange={(e) => {
                    setPorts({ ...ports, [f.key]: e.target.value });
                    reset();
                  }}
                  className={`${input} w-32 font-mono`}
                />
              </label>
            ))}
          </div>
        </fieldset>

        <fieldset className="flex flex-col gap-2">
          <legend className="text-fg-muted">HTTPS port for sites</legend>
          <label className="flex items-center gap-2">
            <input
              type="radio"
              name="https-port"
              checked={httpsAuto}
              onChange={() => {
                setHttpsAuto(true);
                reset();
              }}
              className="accent-accent"
            />
            Automatic: 443 when the web server uses port 80, otherwise 8443
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="https-port"
                checked={!httpsAuto}
                onChange={() => {
                  setHttpsAuto(false);
                  reset();
                }}
                className="accent-accent"
              />
              Use port
            </label>
            <input
              type="number"
              min={1}
              max={65535}
              aria-label="HTTPS port"
              disabled={httpsAuto}
              required={!httpsAuto}
              value={httpsPort}
              onChange={(e) => {
                setHttpsPort(e.target.value);
                reset();
              }}
              className={`${input} w-32 font-mono disabled:opacity-50`}
            />
          </div>
          <p className="text-fg-muted">
            Applies to a web server&apos;s first HTTPS site. A web server that already serves HTTPS sites
            keeps their port.
          </p>
        </fieldset>
        {footer}
      </form>

      <div className="flex flex-col gap-2">
        <h3 className="text-fg-muted">Existing services</h3>
        {services.status === "pending" && <p role="status">Loading services…</p>}
        {services.status === "error" && <ErrorPanel error={services.error} onRetry={services.reload} />}
        {services.status === "success" &&
          (services.data.length === 0 ? (
            <p className="text-fg-muted">No services yet.</p>
          ) : (
            <>
              <ul className="flex flex-col gap-1">
                {services.data.map((s) => (
                  <li key={s.id} className="flex flex-wrap gap-x-3">
                    <Link to={`/services/${s.id}`} className="text-accent hover:underline">
                      {s.instanceName}
                    </Link>
                    <span className="font-mono text-fg-muted">
                      {s.port ?? "no port"}
                      {s.adminPort !== null ? ` / ${s.adminPort}` : ""}
                    </span>
                  </li>
                ))}
              </ul>
              <p className="text-fg-muted">
                To change a service&apos;s ports, stop it and use Change ports on its page.
              </p>
            </>
          ))}
      </div>
    </SettingsSection>
  );
}

const LOGIN_TEXT: Record<LaunchAtLogin, string> = {
  off: "Off",
  on: "On",
  other_copy: "On, but for a different copy of devluma.exe",
};

/** Settings → Startup behavior: start with Windows, and services to start with Devluma. */
export function StartupSettings({ view, save, onChanged }: PreferencesProps & { onChanged: () => void }) {
  const [selected, setSelected] = useState<string[]>(view.preferences.startServices);
  const { run, footer, reset } = useSaver(save);
  const services = useAsync(ipc.listServices);
  const [login, setLogin] = useState<{ busy: boolean; error: AppError | null }>({ busy: false, error: null });

  const toggleLogin = (enabled: boolean) => {
    setLogin({ busy: true, error: null });
    ipc.setLaunchAtLogin(enabled).then(
      () => {
        setLogin({ busy: false, error: null });
        onChanged();
      },
      (e: unknown) => setLogin({ busy: false, error: toAppError(e) }),
    );
  };

  return (
    <SettingsSection id="startup-heading" title="Startup behavior">
      <div className="flex flex-col gap-2">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={view.launchAtLogin === "on"}
            disabled={!view.launchAtLoginSupported || login.busy}
            onChange={(e) => toggleLogin(e.target.checked)}
            className="accent-accent"
          />
          Start Devluma when I sign in to Windows (minimized)
        </label>
        <p className="text-fg-muted">
          Currently: {LOGIN_TEXT[view.launchAtLogin]}. This is a per-user entry in Windows&apos; startup
          programs; you can also turn it off in Task Manager.
        </p>
        {!view.launchAtLoginSupported && (
          <p className="text-fg-muted">Only available in installed builds, not in development builds.</p>
        )}
        {view.launchAtLogin === "other_copy" && (
          <p className="text-warning">
            Windows will start another devluma.exe. Turn this on again to start this copy instead.
          </p>
        )}
        {login.error && <ErrorPanel error={login.error} />}
      </div>

      <form onSubmit={run({ startServices: selected })} className="flex flex-col gap-2">
        <fieldset>
          <legend className="text-fg-muted">Start these services when Devluma opens</legend>
          {services.status === "pending" && <p role="status">Loading services…</p>}
          {services.status === "error" && <ErrorPanel error={services.error} onRetry={services.reload} />}
          {services.status === "success" &&
            (services.data.length === 0 ? (
              <p className="mt-1 text-fg-muted">No services yet.</p>
            ) : (
              <div className="mt-2 flex flex-col gap-1">
                {services.data.map((s) => (
                  <label key={s.id} className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={selected.includes(s.id)}
                      onChange={(e) => {
                        setSelected(
                          e.target.checked ? [...selected, s.id] : selected.filter((id) => id !== s.id),
                        );
                        reset();
                      }}
                      className="accent-accent"
                    />
                    {s.instanceName}
                  </label>
                ))}
              </div>
            ))}
          <p className="mt-1 text-fg-muted">
            Each start shows up in Operations like a manual start. Devluma still stops its services when it
            closes.
          </p>
        </fieldset>
        {footer}
      </form>
    </SettingsSection>
  );
}

/** Settings → Downloads: connection and stalled-download timeouts. */
export function DownloadSettings({ view, save }: PreferencesProps) {
  const t = view.preferences.downloadTimeouts;
  const [connect, setConnect] = useState(String(t.connectSecs));
  const [idle, setIdle] = useState(String(t.idleSecs));
  const { run, footer, reset } = useSaver(save);

  return (
    <SettingsSection id="downloads-heading" title="Downloads">
      <p className="text-fg-muted">
        Packages are downloaded only from the hosts listed in the catalog, over HTTPS, and checked against
        their SHA-256 before anything is unpacked. Raise these on slow or unreliable connections.
      </p>
      <form
        onSubmit={run({ downloadTimeouts: { connectSecs: Number(connect), idleSecs: Number(idle) } })}
        className="flex flex-col gap-3"
      >
        <div className="flex flex-wrap gap-4">
          <label className="flex flex-col gap-1">
            <span>Connection timeout (seconds, 5–300)</span>
            <input
              type="number"
              min={5}
              max={300}
              required
              value={connect}
              onChange={(e) => {
                setConnect(e.target.value);
                reset();
              }}
              className={`${input} w-32 font-mono`}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span>Give up after no data for (seconds, 10–600)</span>
            <input
              type="number"
              min={10}
              max={600}
              required
              value={idle}
              onChange={(e) => {
                setIdle(e.target.value);
                reset();
              }}
              className={`${input} w-32 font-mono`}
            />
          </label>
        </div>
        {footer}
      </form>
    </SettingsSection>
  );
}
