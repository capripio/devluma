import { createContext, useContext, type ReactNode } from "react";
import { ErrorPanel } from "../components/ErrorPanel";
import { ipc } from "../lib/ipc";
import { useAsync } from "../lib/useAsync";
import type { StartupStatus } from "../types/dto";
import { RecoveryPage } from "./RecoveryPage";

// `null` when running outside the desktop app (no backend to ask).
const StartupContext = createContext<StartupStatus | null>(null);

export function useStartupStatus(): StartupStatus | null {
  return useContext(StartupContext);
}

/**
 * Blocks the app until the backend reports whether its metadata database opened.
 * If it didn't, only the recovery page is shown — nothing that could write to it.
 */
export function StartupGate({ children }: { children: ReactNode }) {
  const status = useAsync(ipc.getStartupStatus);

  if (status.status === "pending") {
    return (
      <p role="status" className="p-6 text-sm text-fg-muted">
        Starting Devluma…
      </p>
    );
  }
  if (status.status === "error") {
    // Outside the desktop shell every page already explains that system actions are off.
    if (status.error.code === "DESKTOP_UNAVAILABLE") {
      return <StartupContext value={null}>{children}</StartupContext>;
    }
    return (
      <main className="mx-auto max-w-2xl p-6">
        <ErrorPanel error={status.error} onRetry={status.reload} />
      </main>
    );
  }
  if (status.data.state === "recovery_required") {
    return <RecoveryPage status={status.data} />;
  }
  return <StartupContext value={status.data}>{children}</StartupContext>;
}
