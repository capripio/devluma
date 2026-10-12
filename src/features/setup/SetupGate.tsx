import { useState, type ReactNode } from "react";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import { SetupWizard } from "./SetupWizard";

/**
 * Shows the first-run wizard until setup is completed or skipped. If the setup status can't
 * be read, the app opens normally (setup is optional; everything is reachable from the pages).
 */
export function SetupGate({ children }: { children: ReactNode }) {
  const status = useAsync(ipc.getSetupStatus);
  const [done, setDone] = useState(false);
  if (status.status === "pending") {
    return (
      <p role="status" className="p-6 text-sm text-fg-muted">
        Starting Devluma…
      </p>
    );
  }
  if (done || status.status === "error" || status.data.completed) return children;
  return <SetupWizard status={status.data} onDone={() => setDone(true)} />;
}
