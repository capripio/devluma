import { CircleX } from "lucide-react";
import type { AppError, ErrorCode } from "../lib/ipc";
import { Button } from "./Button";
import { CopyButton } from "./CopyButton";

// Headline + next-step guidance per category (app_flow.md §13). The backend's safe
// message carries the specifics (port, path, PID, ...).
const GUIDANCE: Record<ErrorCode, { title: string; hint: string }> = {
  NETWORK_ERROR: {
    title: "Network problem",
    hint: "Check your connection or the package source, then retry.",
  },
  INTEGRITY_ERROR: {
    title: "Download failed verification",
    hint: "The file did not match its expected checksum or signature, so it was not installed.",
  },
  PERMISSION_DENIED: {
    title: "Permission needed",
    hint: "This action needs a narrowly scoped permission. Review it and retry.",
  },
  PORT_IN_USE: {
    title: "Port already in use",
    hint: "Choose another port or stop the other program yourself.",
  },
  CONFIG_INVALID: {
    title: "Configuration is invalid",
    hint: "The previous configuration was kept. Review the validator output below.",
  },
  PROCESS_EXITED: { title: "Process exited", hint: "Check the exit code and recent logs." },
  DISK_SPACE_LOW: {
    title: "Not enough disk space",
    hint: "Free up space or choose another storage location.",
  },
  DATA_DIRECTORY_EXISTS: {
    title: "Existing data found",
    hint: "Devluma will not overwrite it. Choose how to proceed explicitly.",
  },
  RECOVERY_REQUIRED: {
    title: "Recovery required",
    hint: "An earlier operation did not finish. Resolve it before starting conflicting work.",
  },
  VALIDATION_ERROR: { title: "Invalid input", hint: "Correct the highlighted values and try again." },
  NOT_FOUND: { title: "No longer exists", hint: "It may have been removed. Refresh and try again." },
  NOT_IMPLEMENTED: {
    title: "Not available yet",
    hint: "This feature has not been implemented in this build.",
  },
  INTERNAL: {
    title: "Unexpected error",
    hint: "Retry. If it keeps happening, export diagnostics from Logs.",
  },
  DESKTOP_UNAVAILABLE: {
    title: "Desktop backend unavailable",
    hint: "Devluma's UI is running outside the desktop app, so system actions are disabled.",
  },
};

interface ErrorPanelProps {
  error: AppError;
  onRetry?: () => void;
  retrying?: boolean;
}

export function ErrorPanel({ error, onRetry, retrying }: ErrorPanelProps) {
  const guidance = GUIDANCE[error.code];
  return (
    <div role="alert" className="flex gap-3 rounded-md border border-danger/40 bg-danger-subtle p-4 text-sm">
      <CircleX aria-hidden className="mt-0.5 size-5 shrink-0 text-danger" />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <p className="font-semibold text-fg">{guidance.title}</p>
        <p className="text-fg">{error.message}</p>
        <p className="text-fg-muted">{guidance.hint}</p>
        <p className="font-mono text-xs text-fg-muted">
          Code: {error.code}
          {error.operationId && (
            <>
              {" · "}Operation: <span className="select-all">{error.operationId}</span>{" "}
              <CopyButton value={error.operationId} label="operation ID" />
            </>
          )}
        </p>
        {error.recoverable && onRetry && (
          <div className="mt-2">
            <Button onClick={onRetry} pending={retrying}>
              Retry
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
