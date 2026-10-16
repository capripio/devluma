import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  createMockAdapter,
  defaultHandlers,
  preferencesView,
  renderApp,
} from "../../test/utils";

function savedPreferences(mock: ReturnType<typeof createMockAdapter>) {
  const call = mock.calls.filter((c) => c.command === "set_preferences").at(-1);
  return (call?.args as { preferences: Record<string, unknown> } | undefined)?.preferences;
}

/** Mock whose set_preferences stores what it receives (suffix normalized), like the backend. */
function storingMock(extra: Record<string, (args?: Record<string, unknown>) => unknown> = {}) {
  let stored = preferencesView();
  return createMockAdapter({
    ...defaultHandlers,
    get_preferences: () => stored,
    set_preferences: (args) => {
      const preferences = (args as { preferences: typeof stored.preferences }).preferences;
      const domainSuffix = preferences.domainSuffix
        .trim()
        .replace(/^\.+|\.+$/g, "")
        .toLowerCase();
      const https = ["test", "localhost", "internal", "example", "invalid", "local"].includes(
        domainSuffix.split(".").at(-1)!,
      );
      stored = {
        ...stored,
        preferences: { ...preferences, domainSuffix },
        suffixAdvice: {
          httpsSupported: https,
          warnings: https ? [] : ["HTTPS won't be available for these sites."],
        },
      };
      return stored;
    },
    ...extra,
  });
}

describe("Settings preferences", () => {
  it("saves a typed domain suffix and the terminal, showing the backend's advice", async () => {
    const mock = storingMock();
    const { container } = await renderApp("/settings/sites-terminal", mock);
    const section = (await screen.findByRole("heading", { name: "Sites & terminal" })).closest("section")!;

    const field = within(section).getByRole("combobox", { name: "Domain suffix for new sites" });
    await userEvent.clear(field);
    await userEvent.type(field, ".Dev");
    expect(section).toHaveTextContent("my-app.dev");
    expect(section).toHaveTextContent("Save to check this suffix.");

    await userEvent.click(within(section).getByRole("radio", { name: "Cmder" }));
    expect(
      within(section).getByRole("link", { name: "Install it from Runtimes & Packages" }),
    ).toHaveAttribute("href", "/runtimes/cmder");
    await userEvent.click(within(section).getByRole("button", { name: "Save" }));
    expect(await within(section).findByText("Saved.")).toBeInTheDocument();
    expect(savedPreferences(mock)).toMatchObject({ domainSuffix: ".Dev", terminal: "cmder" });
    expect(await within(section).findByText("HTTPS won't be available for these sites.")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("fills the field from a suggestion", async () => {
    const mock = storingMock();
    await renderApp("/settings/sites-terminal", mock);
    const section = (await screen.findByRole("heading", { name: "Sites & terminal" })).closest("section")!;
    await userEvent.click(within(section).getByRole("button", { name: ".local" }));
    expect(within(section).getByRole("combobox", { name: "Domain suffix for new sites" })).toHaveValue(
      "local",
    );
    await userEvent.click(within(section).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(savedPreferences(mock)).toMatchObject({ domainSuffix: "local" }));
  });

  it("saves default ports and a fixed HTTPS port without touching other preferences", async () => {
    const mock = storingMock();
    await renderApp("/settings/ports", mock);
    const section = (await screen.findByRole("heading", { name: "Ports" })).closest("section")!;

    const nginx = within(section).getByRole("spinbutton", { name: "Nginx" });
    await userEvent.clear(nginx);
    await userEvent.type(nginx, "80");
    await userEvent.click(within(section).getByRole("radio", { name: "Use port" }));
    const https = within(section).getByRole("spinbutton", { name: "HTTPS port" });
    await userEvent.clear(https);
    await userEvent.type(https, "9443");
    await userEvent.click(within(section).getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(savedPreferences(mock)).toMatchObject({
        httpsPort: 9443,
        domainSuffix: "test",
        defaultPorts: expect.objectContaining({ nginx: 80, mariadb: 3306 }) as unknown,
      }),
    );
  });

  it("keeps earlier saves when another section saves later", async () => {
    const mock = storingMock();
    await renderApp("/settings/sites-terminal", mock);
    const sites = (await screen.findByRole("heading", { name: "Sites & terminal" })).closest("section")!;
    await userEvent.click(within(sites).getByRole("button", { name: ".localhost" }));
    await userEvent.click(within(sites).getByRole("button", { name: "Save" }));
    await within(sites).findByText("Saved.");

    await userEvent.click(screen.getByRole("link", { name: "Downloads" }));
    const downloads = (await screen.findByRole("heading", { name: "Downloads" })).closest("section")!;
    const connect = within(downloads).getByRole("spinbutton", { name: /Connection timeout/ });
    await userEvent.clear(connect);
    await userEvent.type(connect, "60");
    await userEvent.click(within(downloads).getByRole("button", { name: "Save" }));
    await within(downloads).findByText("Saved.");

    expect(savedPreferences(mock)).toMatchObject({
      domainSuffix: "localhost",
      downloadTimeouts: { connectSecs: 60, idleSecs: 30 },
    });
  });

  it("shows the backend's validation error", async () => {
    const mock = storingMock({
      set_preferences: () => {
        throw {
          code: "VALIDATION_ERROR",
          message: "Mailpit's SMTP and web interface ports must be different.",
          recoverable: false,
        };
      },
    });
    await renderApp("/settings/ports", mock);
    const section = (await screen.findByRole("heading", { name: "Ports" })).closest("section")!;
    await userEvent.click(within(section).getByRole("button", { name: "Save" }));
    expect(await within(section).findByText(/must be different/)).toBeInTheDocument();
  });

  it("chooses services to start with Devluma and toggles start with Windows", async () => {
    let login = "off";
    const mock = storingMock({
      list_services: () => [
        {
          id: "svc-1",
          packageInstallationId: "i-ng",
          instanceName: "web",
          serviceType: "web_server",
          lifecycleAdapter: "nginx",
          desiredState: "stopped",
          observedState: "stopped",
          port: 8080,
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
          updatedAt: "2026-10-11T10:00:00.000Z",
          role: "web_server",
          packageId: "nginx",
          packageName: "Nginx",
          version: "1.30.5",
        },
      ],
      set_launch_at_login: (args) => {
        login = (args as { enabled: boolean }).enabled ? "on" : "off";
        return login;
      },
    });
    const base = mock.adapter.invoke.bind(mock.adapter);
    mock.adapter.invoke = async (command, args) => {
      const r = await base(command, args);
      return command === "get_preferences" ? { ...(r as object), launchAtLogin: login } : r;
    };
    await renderApp("/settings/startup", mock);
    const section = (await screen.findByRole("heading", { name: "Startup behavior" })).closest("section")!;

    await userEvent.click(within(section).getByRole("checkbox", { name: /Start Devluma when I sign in/ }));
    await waitFor(() => expect(section).toHaveTextContent("Currently: On."));

    await userEvent.click(await within(section).findByRole("checkbox", { name: "web" }));
    await userEvent.click(within(section).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(savedPreferences(mock)).toMatchObject({ startServices: ["svc-1"] }));
  });

  it("lists storage folders with their sizes", async () => {
    await renderApp(
      "/settings/storage",
      createMockAdapter({
        ...defaultHandlers,
        get_storage_usage: () => ({
          root: String.raw`C:\Devluma`,
          folders: [{ key: "packages", path: String.raw`C:\Devluma\bin`, bytes: 3 * 1024 * 1024, files: 12 }],
        }),
      }),
    );
    const section = (await screen.findByRole("heading", { name: "Storage" })).closest("section")!;
    expect(await within(section).findByText("Downloaded tools")).toBeInTheDocument();
    expect(section).toHaveTextContent("3.0 MB");
    expect(section).toHaveTextContent("(12 files)");
  });
});
