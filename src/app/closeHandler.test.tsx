import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, preferencesView, renderApp } from "../test/utils";

const service = (id: string, name: string, observedState: string) => ({
  id,
  packageInstallationId: `i-${id}`,
  instanceName: name,
  serviceType: "web_server",
  lifecycleAdapter: "nginx",
  desiredState: "running",
  observedState,
  port: 80,
  bindAddress: "127.0.0.1",
  configPath: null,
  dataPath: null,
  pid: null,
  processStartedAt: null,
  healthStatus: "unknown",
  lastExitCode: null,
  lastErrorCode: null,
  jobName: null,
  adminPort: null,
  updatedAt: "2026-10-21T10:00:00.000Z",
  role: "web_server",
  packageId: "nginx",
  packageName: "Nginx",
  version: "1.30.5",
  portConflicts: [],
});

describe("closing the window", () => {
  it("takes over the close button only once it is listening", async () => {
    const mock = createMockAdapter(defaultHandlers);
    await renderApp("/", mock);
    await waitFor(() => expect(mock.calls.some((c) => c.command === "register_close_handler")).toBe(true));
  });

  it("asks what to do, and can remember keeping Devluma in the notification area", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_preferences: () => preferencesView(),
      set_preferences: () => preferencesView({ closeAction: "tray" }),
      hide_main_window: () => null,
    });
    const { container } = await renderApp("/", mock);
    await waitFor(() => expect(mock.calls.some((c) => c.command === "register_close_handler")).toBe(true));

    act(() => mock.emit("app://close-requested", null));
    const dialog = await screen.findByRole("dialog", { name: "Close Devluma?" });
    expect(await axeViolations(container)).toEqual([]);
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Remember my choice" }));
    await userEvent.click(
      within(dialog).getByRole("button", { name: /Keep running in the notification area/ }),
    );

    await waitFor(() => expect(mock.calls.some((c) => c.command === "hide_main_window")).toBe(true));
    expect(screen.queryByRole("dialog", { name: "Close Devluma?" })).not.toBeInTheDocument();
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "set_preferences")?.args).toMatchObject({
        preferences: { closeAction: "tray" },
      }),
    );
    // Nothing was stopped.
    expect(mock.calls.some((c) => c.command === "quit_app")).toBe(false);
  });

  it("cancels without doing anything", async () => {
    const mock = createMockAdapter(defaultHandlers);
    await renderApp("/", mock);
    await waitFor(() => expect(mock.calls.some((c) => c.command === "register_close_handler")).toBe(true));
    act(() => mock.emit("app://close-requested", null));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(mock.calls.some((c) => ["quit_app", "hide_main_window"].includes(c.command))).toBe(false);
  });

  it("shows every service stopping while it quits", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [service("s1", "web", "running"), service("s2", "db", "stopped")],
      quit_app: () => null,
    });
    await renderApp("/", mock);
    await waitFor(() => expect(mock.calls.some((c) => c.command === "register_close_handler")).toBe(true));

    act(() => mock.emit("app://quit-requested", null));
    const screenDialog = await screen.findByRole("dialog", { name: "Stopping services" });
    await waitFor(() => expect(mock.calls.some((c) => c.command === "quit_app")).toBe(true));
    expect(within(screenDialog).getByText("web")).toBeInTheDocument();
    act(() =>
      mock.emit("service://state", {
        serviceId: "s1",
        observedState: "stopping",
        healthStatus: "unknown",
        pid: 1,
        lastExitCode: null,
        lastErrorCode: null,
      }),
    );
    expect(await within(screenDialog).findByText("Stopping")).toBeInTheDocument();
    expect(within(screenDialog).queryByRole("button", { name: "Quit anyway" })).not.toBeInTheDocument();
  });

  it("offers Quit anyway when a service doesn't stop cleanly", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [service("s1", "web", "failed")],
      quit_app: () => null,
      quit_app_now: () => null,
    });
    await renderApp("/", mock);
    await waitFor(() => expect(mock.calls.some((c) => c.command === "register_close_handler")).toBe(true));
    act(() => mock.emit("app://quit-requested", null));
    const screenDialog = await screen.findByRole("dialog", { name: "Stopping services" });
    expect(await within(screenDialog).findByText(/picks them up again next time/)).toBeInTheDocument();
    await userEvent.click(within(screenDialog).getByRole("button", { name: "Quit anyway" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "quit_app_now")).toBe(true));
  });
});
