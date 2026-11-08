import * as Tabs from "@radix-ui/react-tabs";
import { CircleCheck, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { ConfigApplied, ConfigTarget, ConfigView } from "../../types/dto";
import { ConfigEditor } from "./ConfigEditor";
import { SettingsForm } from "./SettingsForm";

const tabClass =
  "border-b-2 border-transparent px-3 py-1.5 text-sm text-fg-muted transition-colors hover:text-fg data-[state=active]:border-accent data-[state=active]:font-medium data-[state=active]:text-fg";

/**
 * Configuration for a PHP version or a service (ADR 0018): friendly settings, and the real
 * files in an editor for everything else. Both save through preview → check → rollback.
 */
export function ConfigPanel({ target, onApplied }: { target: ConfigTarget; onApplied?: () => void }) {
  const loaded = useAsync(() => ipc.getConfig(target));
  const [view, setView] = useState<ConfigView | null>(null);
  const [result, setResult] = useState<ConfigApplied | null>(null);
  const current = view ?? (loaded.status === "success" ? loaded.data : null);

  if (!current) {
    if (loaded.status === "error") return <ErrorPanel error={loaded.error} onRetry={loaded.reload} />;
    return <LoadingState label="Loading configuration…" shape="panel" rows={6} />;
  }
  const applied = (r: ConfigApplied) => {
    setView(r.view);
    setResult(r);
    onApplied?.();
  };
  const reload = () => {
    setView(null);
    setResult(null);
    loaded.reload();
  };
  const hasSettings = current.settings.length > 0 || current.extensions !== null;

  return (
    <div className="flex flex-col gap-4">
      {result && <AppliedNotice result={result} onDismiss={() => setResult(null)} />}
      <Tabs.Root defaultValue={hasSettings ? "settings" : "advanced"} className="flex flex-col gap-4">
        <Tabs.List aria-label="Configuration view" className="flex gap-1 border-b border-border">
          <Tabs.Trigger value="settings" className={tabClass}>
            Settings
          </Tabs.Trigger>
          <Tabs.Trigger value="advanced" className={tabClass}>
            Advanced
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="settings">
          <SettingsForm
            key={JSON.stringify(current.settings)}
            target={target}
            view={current}
            onApplied={applied}
          />
        </Tabs.Content>
        <Tabs.Content value="advanced">
          <ConfigEditor target={target} files={current.files} onApplied={applied} onReload={reload} />
        </Tabs.Content>
      </Tabs.Root>
    </div>
  );
}

/** What happened on save, the tool's remarks, and restarts still needed. */
function AppliedNotice({ result, onDismiss }: { result: ConfigApplied; onDismiss: () => void }) {
  const [restarting, setRestarting] = useState<Record<string, "pending" | "started">>({});
  const [error, setError] = useState<AppError | null>(null);
  const restart = async (serviceId: string) => {
    setError(null);
    setRestarting((r) => ({ ...r, [serviceId]: "pending" }));
    try {
      await ipc.restartService(serviceId);
      setRestarting((r) => ({ ...r, [serviceId]: "started" }));
    } catch (e) {
      setError(toAppError(e));
      setRestarting((r) => {
        const next = { ...r };
        delete next[serviceId];
        return next;
      });
    }
  };
  return (
    <section
      aria-label="Saved"
      className="flex flex-col gap-2 rounded-md border border-success/40 bg-success-subtle p-3 text-sm"
    >
      <p role="status" className="flex gap-2">
        <CircleCheck aria-hidden className="mt-0.5 size-4 shrink-0 text-success" />
        <span className="flex-1">{result.message}</span>
        <button type="button" className="text-xs text-accent hover:underline" onClick={onDismiss}>
          Dismiss
        </button>
      </p>
      {result.warnings.length > 0 && (
        <ul className="flex flex-col gap-1 pl-6 text-xs">
          {result.warnings.map((w) => (
            <li key={w} className="flex gap-1.5">
              <TriangleAlert aria-hidden className="size-3.5 shrink-0 text-warning" />
              <span className="font-mono">{w}</span>
            </li>
          ))}
        </ul>
      )}
      {result.restart.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 pl-6">
          {result.restart.map((r) =>
            restarting[r.serviceId] === "started" ? (
              <span key={r.serviceId} className="text-xs text-fg-muted">
                Restarting {r.name}. Progress is in Operations.
              </span>
            ) : (
              <Button
                key={r.serviceId}
                pending={restarting[r.serviceId] === "pending"}
                onClick={() => void restart(r.serviceId)}
              >
                Restart {r.name} now
              </Button>
            ),
          )}
        </div>
      )}
      {error && <ErrorPanel error={error} />}
    </section>
  );
}
