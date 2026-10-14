import { useState } from "react";
import { Button } from "../../components/Button";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import { terminalOpenedMessage } from "./terminal";

/** Opens the terminal chosen in Settings in the home folder, with the default runtimes. */
export function OpenTerminalButton({ align = "end" }: { align?: "start" | "end" }) {
  const [status, setStatus] = useState<{ message: string; error: boolean } | null>(null);

  const open = () => {
    setStatus({ message: "Opening…", error: false });
    ipc.openTerminal().then(
      (r) => setStatus({ message: terminalOpenedMessage(r), error: false }),
      (e: unknown) => setStatus({ message: toAppError(e).message, error: true }),
    );
  };

  return (
    <div className={`flex flex-col gap-1 ${align === "end" ? "items-end" : "items-start"}`}>
      <Button onClick={open}>Open terminal</Button>
      {status && (
        <span
          role="status"
          className={`max-w-sm text-xs ${align === "end" ? "text-right" : ""} ${status.error ? "text-danger" : "text-fg-muted"}`}
        >
          {status.message}
        </span>
      )}
    </div>
  );
}
