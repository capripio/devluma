import * as Dialog from "@radix-ui/react-dialog";
import { EyeOff, Power } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { Button } from "../components/Button";
import { ErrorPanel } from "../components/ErrorPanel";
import { ServiceStatusBadge } from "../components/StatusBadge";
import { events, ipc } from "../lib/ipc";
import { toAppError, type AppError } from "../lib/ipc/errors";
import type { CloseAction } from "../types/dto";
import { useServices } from "../features/services/useServices";

/** How long services get before "Quit anyway" is offered. */
const QUIT_ANYWAY_AFTER_MS = 15_000;

/**
 * What the window's close button does (`lifecycle.rs`): the backend asks here when the
 * choice is "ask", and sends quits here so the user sees every service stop. Until this is
 * mounted the window closes as before and services stop silently on exit.
 */
export function CloseHandler() {
  const [mode, setMode] = useState<"idle" | "asking" | "quitting">("idle");

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void)[] = [];
    Promise.all([
      events.onCloseRequested(() => setMode((m) => (m === "quitting" ? m : "asking"))),
      events.onQuitRequested(() => setMode("quitting")),
    ])
      .then((fns) => {
        if (disposed) return fns.forEach((f) => f());
        unlisten = fns;
        // Only now can the close button be handed over to this component.
        return ipc.registerCloseHandler();
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten.forEach((f) => f());
    };
  }, []);

  if (mode === "asking")
    return (
      <CloseDialog
        onCancel={() => setMode("idle")}
        onChoose={(action) => {
          if (action === "quit") setMode("quitting");
          else {
            setMode("idle");
            ipc.hideMainWindow().catch(() => {});
          }
        }}
      />
    );
  if (mode === "quitting") return <QuittingScreen onBack={() => setMode("idle")} />;
  return null;
}

function CloseDialog({
  onCancel,
  onChoose,
}: {
  onCancel: () => void;
  onChoose: (action: Exclude<CloseAction, "ask">) => void;
}) {
  const [remember, setRemember] = useState(false);
  const choose = (action: Exclude<CloseAction, "ask">) => {
    if (remember) {
      // Best effort: the choice still applies this time if saving fails.
      ipc
        .getPreferences()
        .then((v) => ipc.setPreferences({ ...v.preferences, closeAction: action }))
        .catch(() => {});
    }
    onChoose(action);
  };

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onCancel()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-[rgb(5_10_22/0.55)]" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 flex w-[min(30rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 rounded-md border border-border-strong bg-surface-raised p-5 text-sm shadow-[0_24px_64px_-12px_rgb(0_0_0/0.5)]">
          <Dialog.Title className="text-lg font-semibold">Close Devluma?</Dialog.Title>
          <Dialog.Description className="text-fg-muted">
            Your sites and databases can keep running in the background, or stop now.
          </Dialog.Description>
          <div className="flex flex-col gap-2">
            <Choice
              icon={<EyeOff aria-hidden className="size-5" />}
              title="Keep running in the notification area"
              detail="The window hides and everything keeps working. Open Devluma again from its icon next to the clock."
              onClick={() => choose("tray")}
            />
            <Choice
              icon={<Power aria-hidden className="size-5" />}
              title="Quit and stop all services"
              detail="Stops every service Devluma started, shows each one stopping, then closes."
              onClick={() => choose("quit")}
            />
          </div>
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
            Remember my choice
          </label>
          <p className="-mt-2 text-xs text-fg-muted">You can change it in Settings → Startup behavior.</p>
          <div className="flex justify-end">
            <Dialog.Close asChild>
              <Button>Cancel</Button>
            </Dialog.Close>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Choice({
  icon,
  title,
  detail,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  detail: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex items-start gap-3 rounded-md border border-border-strong/70 bg-surface px-3 py-3 text-left transition-colors hover:border-accent hover:bg-accent-subtle"
    >
      <span className="mt-0.5 text-fg-muted">{icon}</span>
      <span className="flex flex-col gap-0.5">
        <span className="font-semibold">{title}</span>
        <span className="text-xs text-fg-muted">{detail}</span>
      </span>
    </button>
  );
}

/**
 * Covers the window while services stop, one row per service with its live state. The
 * backend exits by itself when they have all stopped.
 */
function QuittingScreen({ onBack }: { onBack: () => void }) {
  const services = useServices();
  const [error, setError] = useState<AppError | null>(null);
  const [offerForce, setOfferForce] = useState(false);

  useEffect(() => {
    ipc.quitApp().catch((e: unknown) => setError(toAppError(e)));
    const timer = setTimeout(() => setOfferForce(true), QUIT_ANYWAY_AFTER_MS);
    return () => clearTimeout(timer);
  }, []);

  const list =
    services.status === "success" ? services.data.filter((s) => s.observedState !== "not_installed") : [];
  const failed = list.some((s) => s.observedState === "failed");

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="quitting-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface p-6"
    >
      <div className="flex w-[min(30rem,100%)] flex-col gap-4 text-sm">
        <h2 id="quitting-title" className="font-display text-2xl font-semibold">
          Stopping services
        </h2>
        <p className="text-fg-muted">Devluma closes by itself once everything has stopped.</p>
        {error ? (
          <>
            <ErrorPanel error={error} />
            <div>
              <Button onClick={onBack}>Back to Devluma</Button>
            </div>
          </>
        ) : (
          <>
            <ul
              aria-live="polite"
              className="flex flex-col divide-y divide-border rounded-md border border-border"
            >
              {list.map((s) => (
                <li key={s.id} className="flex items-center justify-between gap-3 px-3 py-2">
                  <span>
                    {s.instanceName} <span className="text-fg-muted">({s.packageName})</span>
                  </span>
                  <ServiceStatusBadge state={s.observedState} />
                </li>
              ))}
              {services.status === "success" && list.length === 0 && (
                <li className="px-3 py-2 text-fg-muted">No services to stop.</li>
              )}
            </ul>
            {(offerForce || failed) && (
              <div className="flex flex-col items-start gap-2 rounded-md border border-warning/40 bg-warning-subtle p-3">
                <p>
                  {failed ? "A service didn't stop cleanly." : "This is taking longer than usual."} You can
                  quit now; services that are still running keep running, and Devluma picks them up again next
                  time it opens.
                </p>
                <Button onClick={() => ipc.quitAppNow().catch((e: unknown) => setError(toAppError(e)))}>
                  Quit anyway
                </Button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
