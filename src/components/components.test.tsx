import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { AppError } from "../lib/ipc/errors";
import { SERVICE_STATES } from "../types/dto";
import { ConfirmChangeDialog } from "./ConfirmChangeDialog";
import { ErrorPanel } from "./ErrorPanel";
import { LoadingState } from "./LoadingState";
import { ServiceStatusBadge } from "./StatusBadge";

describe("ServiceStatusBadge", () => {
  it.each(SERVICE_STATES)("renders a text label for %s (not color alone)", (state) => {
    const { container } = render(<ServiceStatusBadge state={state} />);
    expect(container.textContent?.trim()).not.toBe("");
  });
});

describe("ErrorPanel", () => {
  it("shows guidance, the safe message and a retry for recoverable errors", async () => {
    const onRetry = vi.fn();
    render(
      <ErrorPanel
        error={
          new AppError({
            code: "PORT_IN_USE",
            message: "Port 80 is used by httpd.exe (PID 42).",
            recoverable: true,
          })
        }
        onRetry={onRetry}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Port already in use");
    expect(screen.getByRole("alert")).toHaveTextContent("PID 42");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("offers no retry for unrecoverable errors", () => {
    render(
      <ErrorPanel
        error={new AppError({ code: "INTEGRITY_ERROR", message: "Hash mismatch.", recoverable: false })}
        onRetry={() => {}}
      />,
    );
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
  });
});

function Harness({ onConfirm }: { onConfirm: () => Promise<void> }) {
  const [open, setOpen] = useState(true);
  return (
    <>
      <span data-testid="state">{open ? "open" : "closed"}</span>
      <ConfirmChangeDialog
        open={open}
        onOpenChange={setOpen}
        title="Apply hosts-file changes?"
        target={"C:\\Windows\\System32\\drivers\\etc\\hosts"}
        impact="+ myapp.test -> 127.0.0.1"
        backup="hosts-20261011.bak"
        confirmLabel="Back up & apply"
        onConfirm={onConfirm}
      />
    </>
  );
}

describe("ConfirmChangeDialog", () => {
  it("shows the exact target and impact", () => {
    render(<Harness onConfirm={async () => {}} />);
    const dialog = screen.getByRole("dialog", { name: "Apply hosts-file changes?" });
    expect(dialog).toHaveTextContent("C:\\Windows\\System32\\drivers\\etc\\hosts");
    expect(dialog).toHaveTextContent("myapp.test");
  });

  it("submits once even when confirm is clicked repeatedly, then closes", async () => {
    let resolve!: () => void;
    const onConfirm = vi.fn(() => new Promise<void>((r) => (resolve = r)));
    render(<Harness onConfirm={onConfirm} />);
    const confirm = screen.getByRole("button", { name: "Back up & apply" });

    await userEvent.click(confirm);
    await userEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(confirm).toBeDisabled();

    resolve();
    await waitFor(() => expect(screen.getByTestId("state")).toHaveTextContent("closed"));
  });

  it("stays open and shows the error when the change fails", async () => {
    const onConfirm = vi.fn(() =>
      Promise.reject({ code: "PERMISSION_DENIED", message: "Elevation was declined.", recoverable: true }),
    );
    render(<Harness onConfirm={onConfirm} />);
    await userEvent.click(screen.getByRole("button", { name: "Back up & apply" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Elevation was declined.");
    expect(screen.getByTestId("state")).toHaveTextContent("open");
    expect(screen.getByRole("button", { name: "Back up & apply" })).toBeEnabled();
  });

  it("traps focus and closes on Escape when idle", async () => {
    render(<Harness onConfirm={async () => {}} />);
    expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement);
    await userEvent.keyboard("{Escape}");
    expect(screen.getByTestId("state")).toHaveTextContent("closed");
  });
});

describe("LoadingState", () => {
  it.each(["table", "panel", "inline"] as const)("announces the label as a status (%s)", (shape) => {
    render(<LoadingState label="Loading projects…" shape={shape} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading projects…");
  });

  it("hides the skeleton outline from assistive technology", () => {
    const { container } = render(<LoadingState label="Loading sites…" columns={3} rows={2} />);
    const outline = container.querySelector("[aria-hidden]");
    expect(outline).not.toBeNull();
    expect(screen.getByRole("status").textContent).toBe("Loading sites…");
  });
});
