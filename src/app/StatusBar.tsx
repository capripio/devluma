import { ChevronUp, CircleAlert, CircleCheck, ListChecks } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";
import { OperationTray } from "../components/OperationTray";
import { useServices } from "../features/services/useServices";
import { ipc } from "../lib/ipc";
import { useAsync } from "../lib/useAsync";
import { describeOperationType, isActive, needsRecovery, useOperations } from "./operations";

const TRAY_ID = "operation-tray";

/** Bottom bar: stack health, the running operation, the operations flyout and the version. */
export function StatusBar() {
  const { operations } = useOperations();
  const active = operations.filter(isActive);
  const recoveryCount = operations.filter(needsRecovery).length;
  const current = active[0];
  const [trayOpen, setTrayOpen] = useState(false);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const info = useAsync(ipc.getAppInfo);

  useEffect(() => {
    if (!trayOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setTrayOpen(false);
        toggleRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [trayOpen]);

  return (
    <footer className="relative flex h-7 shrink-0 items-center gap-4 border-t border-border bg-surface-sunken px-3 text-xs text-fg-muted">
      <StackHealth />
      {current && (
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate">
            {describeOperationType(current.operationType)}
            {current.phase ? ` — ${current.phase}` : ""}
          </span>
          {current.total !== undefined && current.current !== undefined && (
            <progress
              className="w-24"
              max={current.total}
              value={current.current}
              aria-label={`Progress for ${describeOperationType(current.operationType)}`}
            />
          )}
        </span>
      )}
      <span className="ml-auto flex items-center gap-3">
        <button
          ref={toggleRef}
          type="button"
          aria-expanded={trayOpen}
          aria-controls={TRAY_ID}
          onClick={() => setTrayOpen((o) => !o)}
          className="inline-flex h-6 items-center gap-1.5 rounded px-2 text-fg transition-colors hover:bg-surface-raised active:bg-border/60"
        >
          <ListChecks aria-hidden className="size-3.5" />
          Operations
          {active.length > 0 && (
            <span className="rounded-full bg-accent px-1.5 text-accent-fg">
              {active.length}
              <span className="sr-only"> active</span>
            </span>
          )}
          {recoveryCount > 0 && (
            <span className="rounded-full bg-warning-subtle px-1.5 text-warning">
              {recoveryCount} need{recoveryCount === 1 ? "s" : ""} recovery
            </span>
          )}
          <ChevronUp
            aria-hidden
            className={`size-3.5 transition-transform ${trayOpen ? "" : "rotate-180"}`}
          />
        </button>
        {info.status === "success" && (
          <span className="font-mono">
            Devluma {info.data.version}
            {info.data.debugBuild && " (debug)"}
          </span>
        )}
      </span>
      <div
        className={`absolute right-2 bottom-full mb-1 w-[min(44rem,calc(100vw-1rem))] overflow-hidden rounded-md border border-border-strong bg-surface-raised shadow-[0_12px_32px_-8px_rgb(0_0_0/0.45)] ${trayOpen ? "" : "hidden"}`}
      >
        <OperationTray id={TRAY_ID} hidden={!trayOpen} />
      </div>
    </footer>
  );
}

/** "4 of 5 services running", plus a link to whatever needs attention. */
function StackHealth() {
  const services = useServices();
  if (services.status !== "success" || services.data.length === 0) return null;
  const list = services.data;
  const running = list.filter((s) => s.observedState === "running").length;
  const attention = list.filter((s) => s.observedState === "failed" || s.observedState === "degraded");
  return (
    <span className="flex items-center gap-3">
      <span className="flex items-center gap-1.5">
        <CircleCheck aria-hidden className="size-3.5 text-success" />
        {running} of {list.length} service{list.length === 1 ? "" : "s"} running
      </span>
      {attention.length > 0 && (
        <Link
          to={attention.length === 1 && attention[0] ? `/services/${attention[0].id}` : "/services"}
          className="flex items-center gap-1.5 rounded px-1 font-semibold text-warning hover:underline"
        >
          <CircleAlert aria-hidden className="size-3.5" />
          {attention.length} need{attention.length === 1 ? "s" : ""} attention
        </Link>
      )}
    </span>
  );
}
