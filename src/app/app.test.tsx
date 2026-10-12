import { act, render, screen, waitFor, within } from "@testing-library/react";
import { ScrollText } from "lucide-react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  createMockAdapter,
  defaultHandlers,
  operation,
  readyStartup,
  renderApp,
} from "../test/utils";
import { PlannedPage } from "../features/planned/PlannedPage";
import { NAV_ITEMS } from "./navigation";

describe("app shell", () => {
  it("renders every navigation section and marks the current one", async () => {
    await renderApp("/");
    const nav = screen.getByRole("navigation", { name: "Main" });
    for (const item of NAV_ITEMS) {
      expect(within(nav).getByRole("link", { name: item.label })).toBeInTheDocument();
    }
    expect(within(nav).getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");
  });

  it("navigates between sections and moves focus to the page", async () => {
    await renderApp("/");
    await userEvent.click(screen.getByRole("link", { name: "Projects" }));
    expect(screen.getByRole("heading", { level: 1, name: "Projects" })).toBeInTheDocument();
    expect(document.activeElement).toBe(screen.getByRole("main"));
  });

  it("redirects unknown routes to Overview", async () => {
    await renderApp("/does-not-exist");
    expect(await screen.findByRole("heading", { level: 1, name: "Overview" })).toBeInTheDocument();
  });

  it.each(NAV_ITEMS.map((i) => i.path))("route %s has no accessibility violations", async (path) => {
    const { container } = await renderApp(path);
    await screen.findByRole("heading", { level: 1 });
    expect(await axeViolations(container)).toEqual([]);
  });
});

describe("startup", () => {
  const recoveryStartup = {
    ...readyStartup,
    state: "recovery_required",
    recovery: undefined,
    error: {
      code: "RECOVERY_REQUIRED",
      message:
        "Devluma's metadata database was written by a different version of Devluma. It was left unchanged.",
      recoverable: false,
    },
  };

  it("shows only the recovery page when the database can't be opened", async () => {
    const { container } = await renderApp(
      "/",
      createMockAdapter({ ...defaultHandlers, get_startup_status: () => recoveryStartup }),
    );
    expect(
      screen.getByRole("heading", { level: 1, name: /needs attention before it can start/ }),
    ).toBeInTheDocument();
    expect(screen.getByText(/different version of Devluma/)).toBeInTheDocument();
    expect(screen.getByText(readyStartup.databasePath)).toBeInTheDocument();
    expect(screen.getByText(/Nothing was deleted/)).toBeInTheDocument();
    // No app shell, and nothing offers to reset or delete the database.
    expect(screen.queryByRole("navigation", { name: "Main" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /reset|delete/i })).not.toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows a retryable error if startup status can't be read", async () => {
    let fail = true;
    await renderApp(
      "/",
      createMockAdapter({
        ...defaultHandlers,
        get_startup_status: () => {
          if (fail) throw { code: "INTERNAL", message: "Backend hiccup.", recoverable: true };
          return readyStartup;
        },
      }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Backend hiccup.");
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("navigation", { name: "Main" })).toBeInTheDocument();
  });

  it("explains what startup reconciliation changed", async () => {
    await renderApp(
      "/",
      createMockAdapter({
        ...defaultHandlers,
        get_startup_status: () => ({
          ...readyStartup,
          recovery: { operationsNeedingRecovery: 1, installationsMarkedBroken: 2, servicesMarkedUnknown: 0 },
        }),
      }),
    );
    const notice = screen.getByRole("region", { name: "Startup checks" });
    expect(notice).toHaveTextContent("1 operation was interrupted");
    expect(notice).toHaveTextContent("2 installed packages are incomplete");
  });
});

describe("planned sections", () => {
  it("disable their primary action and say why, without claiming success", () => {
    render(
      <PlannedPage
        title="Example"
        description="Not built yet."
        icon={ScrollText}
        emptyTitle="Nothing here"
        emptyDescription="Coming later."
        actionLabel="Do the thing"
      />,
    );
    const action = screen.getByRole("button", { name: "Do the thing" });
    expect(action).toBeDisabled();
    expect(action).toHaveAccessibleDescription(/not available yet/i);
  });
});

describe("overview", () => {
  it("shows app details from the backend", async () => {
    await renderApp("/");
    expect(await screen.findByText(/0\.1\.0/)).toBeInTheDocument();
    expect(screen.getByText(/windows \/ x86_64/)).toBeInTheDocument();
  });

  it("shows a safe error with retry when the backend fails, and recovers", async () => {
    let fail = true;
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_app_info: () => {
        if (fail) throw { code: "INTERNAL", message: "Backend hiccup.", recoverable: true };
        return defaultHandlers.get_app_info!();
      },
    });
    await renderApp("/", mock);
    expect(await screen.findByRole("alert")).toHaveTextContent("Backend hiccup.");

    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText(/0\.1\.0/)).toBeInTheDocument();
  });

  it("lists recent operations from the journal", async () => {
    await renderApp(
      "/",
      createMockAdapter({
        ...defaultHandlers,
        list_operations: () => [
          operation({ operationId: "a", status: "succeeded", startedAt: "2026-10-11T09:00:00.000Z" }),
          operation({
            operationId: "b",
            operationType: "switch_runtime",
            status: "failed",
            safeMessage: "php.exe -v did not report 8.4",
            startedAt: "2026-10-11T10:00:00.000Z",
          }),
        ],
      }),
    );
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("Switch runtime");
    expect(rows[0]).toHaveTextContent("Failed");
    expect(rows[0]).toHaveTextContent("php.exe -v did not report 8.4");
    expect(rows[1]).toHaveTextContent("Install package");
    expect(rows[1]).toHaveTextContent("Complete");
  });
});

describe("operation tray", () => {
  it("shows live operation progress and keeps it across navigation", async () => {
    const { mock } = await renderApp("/");
    const toggle = screen.getByRole("button", { name: /Operations/ });
    await userEvent.click(toggle);
    const tray = screen.getByRole("region", { name: "Operations" });
    expect(tray).toHaveTextContent("No operations yet");

    act(() =>
      mock.emit("operation://progress", operation({ phase: "Downloading PHP 8.4", current: 3, total: 10 })),
    );
    expect(tray).toHaveTextContent("Downloading PHP 8.4");
    expect(toggle).toHaveTextContent("1 active");

    await userEvent.click(screen.getByRole("link", { name: "Services" }));
    expect(screen.getByRole("region", { name: "Operations" })).toHaveTextContent("Downloading PHP 8.4");
  });

  it("shows interrupted operations from the journal after a restart", async () => {
    await renderApp(
      "/",
      createMockAdapter({
        ...defaultHandlers,
        list_operations: () => [operation({ status: "needs_recovery" })],
      }),
    );
    const toggle = screen.getByRole("button", { name: /Operations/ });
    await waitFor(() => expect(toggle).toHaveTextContent("1 needs recovery"));
    await userEvent.click(toggle);
    const tray = screen.getByRole("region", { name: "Operations" });
    expect(tray).toHaveTextContent("Needs recovery");
    expect(tray).toHaveTextContent("Interrupted when Devluma closed");
  });

  it("prefers a live event over a stale journal row for the same operation", async () => {
    let resolveList!: (ops: unknown[]) => void;
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_operations: () => new Promise((r) => (resolveList = r)),
    });
    await renderApp("/", mock);
    act(() => mock.emit("operation://progress", operation({ status: "succeeded" })));
    await act(async () => resolveList([operation({ status: "running" })]));

    await userEvent.click(screen.getByRole("button", { name: /Operations/ }));
    expect(screen.getByRole("region", { name: "Operations" })).toHaveTextContent("Complete");
  });
});

describe("settings", () => {
  it("applies and remembers the theme choice", async () => {
    await renderApp("/settings");
    await userEvent.click(screen.getByRole("radio", { name: "Dark" }));
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    expect(localStorage.getItem("devluma.theme")).toBe("dark");

    await userEvent.click(screen.getByRole("radio", { name: "Match Windows" }));
    expect(document.documentElement).not.toHaveAttribute("data-theme");
  });
});

describe("accessibility sweep", () => {
  // Every section with its default (empty) data: no axe violations, exactly one h1, and the
  // skip link is the first thing keyboard users reach.
  it.each(NAV_ITEMS.map((item) => [item.label, item.path]))(
    "%s has no a11y violations",
    async (_label, path) => {
      const { container } = await renderApp(path);
      await waitFor(() =>
        expect(screen.queryAllByRole("status").filter((s) => /Loading/.test(s.textContent ?? ""))).toEqual(
          [],
        ),
      );
      expect(await axeViolations(container)).toEqual([]);
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    },
  );

  it("keyboard users can skip straight to the content", async () => {
    await renderApp("/services");
    await userEvent.tab();
    const skip = screen.getByRole("link", { name: "Skip to content" });
    expect(skip).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(document.activeElement?.id).toBe(skip.getAttribute("href")?.slice(1)));
  });
});
