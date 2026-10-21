import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  catalogView,
  createMockAdapter,
  defaultHandlers,
  operation,
  packageView,
  preferencesView,
  renderApp,
  chooseFromMenu,
  versionView,
} from "../../test/utils";

const service = (overrides: Record<string, unknown> = {}) => ({
  id: "s1",
  packageInstallationId: "i-ng",
  instanceName: "web",
  serviceType: "web_server",
  lifecycleAdapter: "nginx",
  desiredState: "stopped",
  observedState: "stopped",
  port: 8080,
  bindAddress: "127.0.0.1",
  configPath: "C:\\Devluma\\services\\s1\\conf\\nginx.conf",
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
  portConflicts: [],
  ...overrides,
});

const installedCatalog = catalogView([
  packageView(
    "nginx",
    [
      versionView("nginx", "1.30.5", {
        installation: {
          id: "i-ng",
          status: "installed",
          installPath: "C:\\p",
          isGlobalDefault: false,
          installedAt: null,
        },
      }),
    ],
    { displayName: "Nginx", category: "web_server" },
  ),
  packageView(
    "mariadb",
    [
      versionView("mariadb", "11.8.9", {
        installation: {
          id: "i-db",
          status: "installed",
          installPath: "C:\\m",
          isGlobalDefault: false,
          installedAt: null,
        },
      }),
    ],
    { displayName: "MariaDB", category: "database" },
  ),
]);

describe("services list", () => {
  it("points to Runtimes when nothing is installed", async () => {
    await renderApp("/services");
    expect(await screen.findByText("No services yet")).toBeInTheDocument();
    await userEvent.click(screen.getAllByRole("button", { name: "Add service" })[0]!);
    expect(await screen.findByRole("link", { name: /Install one from Runtimes/ })).toHaveAttribute(
      "href",
      "/runtimes",
    );
  });

  it("groups services, shows only backend-reported state, and follows live events", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [
        service(),
        service({ id: "s2", instanceName: "db", role: "database", packageName: "MariaDB", port: 3306 }),
      ],
      start_service: () => ({ operationId: "op-1" }),
    });
    const { container } = await renderApp("/services", mock);
    expect(await screen.findByRole("heading", { name: "Web servers" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Databases" })).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(screen.getByRole("button", { name: "Start web" }));
    expect(mock.calls.find((c) => c.command === "start_service")?.args).toEqual({ serviceId: "s1" });
    // Not optimistic: still "Stopped" until the backend says otherwise.
    const webRow = screen.getByRole("link", { name: "web" }).closest("tr")!;
    expect(webRow).toHaveTextContent("Stopped");

    const status = {
      serviceId: "s1",
      healthStatus: "unknown",
      pid: 42,
      lastExitCode: null,
      lastErrorCode: null,
    };
    act(() => mock.emit("service://state", { ...status, observedState: "starting" }));
    expect(webRow).toHaveTextContent("Starting");
    expect(within(webRow).queryByRole("button", { name: "Start web" })).not.toBeInTheDocument();
    act(() => mock.emit("service://state", { ...status, observedState: "running", healthStatus: "healthy" }));
    expect(webRow).toHaveTextContent("Running");
    expect(webRow).toHaveTextContent("Accepting connections");
    // One visible action per row; Restart is under More.
    expect(within(webRow).getByRole("button", { name: "Stop web" })).toBeEnabled();
    expect(within(webRow).queryByRole("button", { name: "Restart web" })).not.toBeInTheDocument();
    await chooseFromMenu("More actions for web", "Restart");
    expect(mock.calls.find((c) => c.command === "restart_service")?.args).toEqual({ serviceId: "s1" });
  });

  it("shows why the last start failed", async () => {
    await renderApp(
      "/services",
      createMockAdapter({
        ...defaultHandlers,
        list_services: () => [service({ lastErrorCode: "PORT_IN_USE" })],
        list_operations: () => [
          operation({
            operationType: "start_service",
            targetId: "service:s1",
            status: "failed",
            safeMessage: "Port 8080 is already used by httpd.exe (PID 1234). Devluma won't touch it.",
          }),
        ],
      }),
    );
    // Scoped to the table: the (hidden) operations tray carries the same message.
    const table = await screen.findByRole("table");
    expect(
      await within(table).findByText(/Port 8080 is already used by httpd.exe \(PID 1234\)/),
    ).toBeInTheDocument();
  });

  it("warns before starting when another program holds the port", async () => {
    await renderApp(
      "/services/s1",
      createMockAdapter({
        ...defaultHandlers,
        list_services: () => [
          service({ port: 80, portConflicts: [{ port: 80, usedBy: "httpd.exe (PID 1234)" }] }),
        ],
      }),
    );
    expect(
      await screen.findByText(/Port 80 is in use by httpd\.exe \(PID 1234\)\. This service can't start/),
    ).toBeInTheDocument();
    expect(screen.getByTitle("Port 80 is in use by httpd.exe (PID 1234).")).toHaveTextContent(
      "Port in use by another program",
    );
  });

  it("checks the port of a new service and names the program using it", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_packages: () => installedCatalog,
      check_port: (args) =>
        args?.port === 80
          ? { port: 80, usedBy: "Windows (PID 4, HTTP.sys — for example IIS)", assignedTo: null }
          : { port: args?.port, usedBy: null, assignedTo: args?.port === 3306 ? "db" : null },
    });
    await renderApp("/services", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add service" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add service" });
    // Nginx defaults to port 80, which Windows' HTTP.sys holds here.
    expect(
      await within(form).findByText(/Port 80 is in use by Windows \(PID 4, HTTP\.sys/),
    ).toBeInTheDocument();
    await userEvent.type(within(form).getByLabelText("Port"), "8090");
    await waitFor(() => expect(within(form).queryByText(/Port 80 is in use/)).not.toBeInTheDocument());
    await userEvent.clear(within(form).getByLabelText("Port"));
    await userEvent.type(within(form).getByLabelText("Port"), "3306");
    expect(
      await within(form).findByText(
        'Port 3306 is already assigned to the service "db". Choose another port.',
      ),
    ).toBeInTheDocument();
  });

  it("creates a database service and is explicit about root and loopback", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_packages: () => installedCatalog,
      create_service: () => ({ operationId: "op-2" }),
    });
    await renderApp("/services", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add service" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add service" });
    await userEvent.selectOptions(within(form).getByLabelText("Installed version"), "i-db");
    expect(form).toHaveTextContent("The root user has no password");
    expect(form).toHaveTextContent("127.0.0.1");
    await userEvent.type(within(form).getByLabelText("Port"), "3307");
    await userEvent.click(within(form).getByRole("button", { name: "Create service" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Add service" })).not.toBeInTheDocument());
    expect(mock.calls.find((c) => c.command === "create_service")?.args).toEqual({
      request: { installationId: "i-db", name: "db", port: 3307, useExistingData: false },
    });
  });

  it("uses the default port chosen in Settings", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_packages: () => installedCatalog,
      get_preferences: () =>
        preferencesView({
          defaultPorts: {
            nginx: 8080,
            apache: 8081,
            mariadb: 3310,
            postgresql: 5432,
            mongodb: 27017,
            mailpit: 1025,
            mailpitWeb: 8025,
          },
        }),
      create_service: () => ({ operationId: "op-2" }),
    });
    await renderApp("/services", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add service" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add service" });
    await userEvent.selectOptions(within(form).getByLabelText("Installed version"), "i-db");
    await userEvent.click(within(form).getByRole("button", { name: "Create service" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "create_service")).toBe(true));
    expect(mock.calls.find((c) => c.command === "create_service")?.args).toMatchObject({
      request: { installationId: "i-db", port: 3310 },
    });
  });

  it("creates Mailpit with both ports and offers its web interface when running", async () => {
    const mailpitCatalog = catalogView([
      packageView(
        "mailpit",
        [
          versionView("mailpit", "1.31.5", {
            installation: {
              id: "i-mp",
              status: "installed",
              installPath: "C:\\mp",
              isGlobalDefault: false,
              installedAt: null,
            },
          }),
        ],
        { displayName: "Mailpit", category: "mail" },
      ),
    ]);
    const mail = service({
      id: "m1",
      instanceName: "mail",
      serviceType: "mail",
      lifecycleAdapter: "mailpit",
      observedState: "running",
      port: 1025,
      adminPort: 8025,
      role: "mail",
      packageId: "mailpit",
      packageName: "Mailpit",
      version: "1.31.5",
      portConflicts: [],
    });
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_packages: () => mailpitCatalog,
      list_services: () => [mail],
      create_service: () => ({ operationId: "op-3" }),
      open_service_ui: () => "http://127.0.0.1:8025/",
    });
    const { container } = await renderApp("/services", mock);
    const group = await screen.findByRole("region", { name: "Mail" });
    expect(group).toHaveTextContent("UI 8025");
    await chooseFromMenu("More actions for mail", "Open web interface");
    await waitFor(() => expect(mock.calls.some((c) => c.command === "open_service_ui")).toBe(true));
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(screen.getAllByRole("button", { name: "Add service" })[0]!);
    const form = await screen.findByRole("form", { name: "Add service" });
    expect(form).toHaveTextContent("Nothing is delivered to real recipients");
    await userEvent.type(within(form).getByLabelText("Web interface port"), "8026");
    await userEvent.click(within(form).getByRole("button", { name: "Create service" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "create_service")).toBe(true));
    expect(mock.calls.find((c) => c.command === "create_service")?.args).toEqual({
      request: { installationId: "i-mp", name: "mail", port: 1025, adminPort: 8026, useExistingData: false },
    });
  });

  it("keeps the form open with the reason when data already exists", async () => {
    await renderApp(
      "/services",
      createMockAdapter({
        ...defaultHandlers,
        list_packages: () => installedCatalog,
        create_service: () => {
          throw {
            code: "DATA_DIRECTORY_EXISTS",
            message: "D:\\data already contains files. Choose an empty folder.",
            recoverable: false,
          };
        },
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add service" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add service" });
    await userEvent.selectOptions(within(form).getByLabelText("Installed version"), "i-db");
    await userEvent.click(within(form).getByRole("button", { name: "Create service" }));
    const alert = await within(form).findByRole("alert");
    expect(alert).toHaveTextContent("Existing data found");
    expect(alert).toHaveTextContent("already contains files");
  });
});

describe("service detail", () => {
  const details = {
    serviceDir: "C:\\Devluma\\services\\s1",
    configPath: "C:\\Devluma\\services\\s1\\conf\\nginx.conf",
    configText: "worker_processes 1;\nlisten 127.0.0.1:8080;",
    dataPath: null,
    logs: [
      { label: "service.log", path: "C:\\l\\service.log", lines: ["started", "password=[REDACTED]"] },
      { label: "error.log", path: "C:\\l\\error.log", lines: [] },
    ],
  };

  it("shows overview, configuration and redacted logs in accessible tabs", async () => {
    const { container } = await renderApp(
      "/services/s1",
      createMockAdapter({
        ...defaultHandlers,
        list_services: () => [service()],
        get_service_details: () => details,
      }),
    );
    expect(await screen.findByRole("heading", { level: 1, name: "web" })).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:8080")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(screen.getByRole("tab", { name: "Configuration" }));
    expect(await screen.findByText(/listen 127\.0\.0\.1:8080;/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("tab", { name: "Logs" }));
    expect(await screen.findByText(/password=\[REDACTED\]/)).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Log file"), "1");
    expect(screen.getByText("This log is empty.")).toBeInTheDocument();
  });

  it("only allows removal when stopped and says the data folder is kept", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [
        service({
          role: "database",
          instanceName: "db",
          dataPath: "D:\\data\\db",
          lifecycleAdapter: "mariadb",
        }),
      ],
      remove_service: () => ({ keptDataPath: "D:\\data\\db" }),
    });
    await renderApp("/services/s1", mock);
    const connection = await screen.findByRole("region", { name: "Connection" });
    expect(within(connection).getByRole("link", { name: "Databases" })).toHaveAttribute("href", "/databases");
    await userEvent.click(screen.getByRole("button", { name: "Remove db" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("D:\\data\\db");
    expect(dialog).toHaveTextContent("is kept");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove service" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "remove_service")).toBe(true));
  });

  it("disables removal while running", async () => {
    await renderApp(
      "/services/s1",
      createMockAdapter({
        ...defaultHandlers,
        list_services: () => [service({ observedState: "running", pid: 42 })],
      }),
    );
    expect(await screen.findByRole("button", { name: "Remove web" })).toBeDisabled();
    expect(screen.getByText("Stop the service before removing it.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save ports" })).toBeDisabled();
    expect(screen.getByText("Stop the service to change its ports.")).toBeInTheDocument();
  });

  it("changes ports of a stopped service", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [service()],
      set_service_ports: () => service({ port: 8090 }),
    });
    await renderApp("/services/s1", mock);
    const form = await screen.findByRole("form", { name: "Ports" });
    const port = within(form).getByLabelText("Port");
    await userEvent.clear(port);
    await userEvent.type(port, "8090");
    await userEvent.click(within(form).getByRole("button", { name: "Save ports" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "set_service_ports")?.args).toEqual({
        serviceId: "s1",
        port: 8090,
        adminPort: null,
      }),
    );
    expect(await within(form).findByText(/new ports from its next start/)).toBeInTheDocument();
  });
});
