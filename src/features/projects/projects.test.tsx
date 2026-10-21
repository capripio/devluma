import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  catalogView,
  createMockAdapter,
  defaultHandlers,
  packageView,
  preferencesView,
  renderApp,
  versionView,
} from "../../test/utils";

const installed = (id: string) => ({
  installation: {
    id,
    status: "installed",
    installPath: `C:\\Devluma\\packages\\php\\${id}`,
    isGlobalDefault: id === "i83",
    installedAt: "2026-10-11T10:00:00.000Z",
  },
});

const catalog = catalogView([
  packageView(
    "php",
    [versionView("php", "8.3.35", installed("i83")), versionView("php", "8.4.26", installed("i84"))],
    {
      displayName: "PHP",
    },
  ),
]);

const project = (overrides: Record<string, unknown> = {}) => ({
  id: "p1",
  name: "Shop",
  rootPath: "D:\\code\\shop",
  projectType: null,
  localDomain: null,
  notes: null,
  isFavorite: false,
  createdAt: "2026-10-11T10:00:00.000Z",
  updatedAt: "2026-10-11T10:00:00.000Z",
  lastOpenedAt: null,
  serviceIds: [],
  startServiceIds: [],
  detection: {
    kind: "laravel",
    label: "Laravel",
    suggestedDocumentRoot: String.raw`D:\code\shop\public`,
    virtualEnvScripts: null,
  },
  runtimes: [
    {
      packageId: "php",
      packageName: "PHP",
      effective: {
        installationId: "i83",
        version: "8.3.35",
        installPath: "C:\\Devluma\\packages\\php\\i83",
        source: "global_default",
      },
    },
  ],
  ...overrides,
});

const svc = (id: string, serviceType: string, overrides: Record<string, unknown> = {}) => ({
  id,
  packageInstallationId: `i-${id}`,
  instanceName: id,
  serviceType,
  lifecycleAdapter: "nginx",
  desiredState: "stopped",
  observedState: "stopped",
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
  updatedAt: "2026-10-11T10:00:00.000Z",
  role: "web_server",
  packageId: "nginx",
  packageName: "Nginx",
  version: "1.30.5",
  portConflicts: [],
  ...overrides,
});

const preview = (overrides: Record<string, unknown> = {}) => ({
  scope: "project",
  packageId: "php",
  packageName: "PHP",
  projectName: "Shop",
  current: project().runtimes[0]!.effective,
  target: {
    installationId: "i84",
    version: "8.4.26",
    installPath: "C:\\Devluma\\packages\\php\\i84",
    source: "project_pin",
  },
  changes: ["Shop: PHP 8.3.35 → PHP 8.4.26."],
  unchanged: [
    "Your Windows PATH and environment variables are not modified.",
    "The default PHP and other projects are not affected.",
  ],
  noOp: false,
  ...overrides,
});

describe("projects", () => {
  it("starts empty with one clear action", async () => {
    await renderApp("/projects");
    expect(await screen.findByText("No projects yet")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Add project" }).length).toBeGreaterThan(0);
  });

  it("adds a project in www and shows backend validation errors inline", async () => {
    let attempts = 0;
    const mock = createMockAdapter({
      ...defaultHandlers,
      create_project: () => {
        attempts += 1;
        if (attempts === 1) {
          throw {
            code: "VALIDATION_ERROR",
            message: "Enter a folder inside www, for example my-app.",
            recoverable: false,
          };
        }
        return { ...project(), runtimes: undefined };
      },
    });
    await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add project" }))[0]!);
    const form = screen.getByRole("form", { name: "Add project" });
    await userEvent.type(within(form).getByLabelText("Name"), "My Shop");
    // The folder follows the name and shows where it ends up.
    expect(within(form).getByLabelText("Folder")).toHaveValue("my-shop");
    expect(within(form).getByText(/Devluma\\www\\my-shop/)).toBeInTheDocument();
    await userEvent.click(within(form).getByLabelText("Also create a site at"));
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("inside www");

    await userEvent.clear(within(form).getByLabelText("Folder"));
    await userEvent.type(within(form).getByLabelText("Folder"), "clients\\shop");
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Add project" })).not.toBeInTheDocument());
    expect(mock.calls.filter((c) => c.command === "create_project").at(-1)?.args).toEqual({
      name: "My Shop",
      folder: "clients\\shop",
      serviceIds: [],
    });
    expect(screen.queryByRole("form", { name: "Add site" })).not.toBeInTheDocument();
  });

  it("asks which services a new project uses, preselecting one of each kind", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [
        svc("web", "web_server"),
        svc("apache", "web_server", { packageName: "Apache httpd", port: 8081 }),
        svc("db", "database", { role: "database", packageName: "MariaDB", port: 3306 }),
        svc("php-8.4", "fast_cgi", { role: "fast_cgi", packageName: "PHP", port: 9000 }),
      ],
      create_project: () => ({ ...project(), runtimes: undefined }),
    });
    await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add project" }))[0]!);
    const form = screen.getByRole("form", { name: "Add project" });
    const services = await within(form).findByRole("group", { name: /Services this project uses/ });
    expect(within(services).getByLabelText(/^web/)).toBeChecked();
    expect(within(services).getByLabelText(/^apache/)).not.toBeChecked();
    expect(within(services).getByLabelText(/^db/)).toBeChecked();
    // PHP backends come with sites; they aren't chosen by hand.
    expect(within(services).queryByLabelText(/^php-8\.4/)).not.toBeInTheDocument();

    await userEvent.click(within(services).getByLabelText(/^db/));
    await userEvent.type(within(form).getByLabelText("Name"), "Shop");
    await userEvent.click(within(form).getByLabelText("Also create a site at"));
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "create_project")?.args).toEqual({
        name: "Shop",
        folder: "shop",
        serviceIds: ["web"],
      }),
    );
  });

  it("starts all of a project's services and names the program holding a port", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project({ serviceIds: ["web", "db"], startServiceIds: ["web", "db"] })],
      list_services: () => [
        svc("web", "web_server", { portConflicts: [{ port: 80, usedBy: "httpd.exe (PID 1234)" }] }),
        svc("db", "database", { role: "database", packageName: "MariaDB", port: 3306 }),
      ],
      start_project_services: () => ({
        started: [{ serviceName: "db", operationId: "op-1" }],
        alreadyRunning: [],
        notStarted: [{ serviceName: "web", message: "Port 80 is already used by httpd.exe (PID 1234)." }],
      }),
      set_project_services: () => null,
    });
    const { container } = await renderApp("/projects", mock);
    const table = await screen.findByRole("table");
    // The taken port is shown before anything is started.
    expect(await within(table).findByText("Port 80 is in use by httpd.exe (PID 1234).")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(table).getByRole("button", { name: "Start all services for Shop" }));
    expect(await within(table).findByText("Starting db.")).toBeInTheDocument();
    expect(
      within(table).getByText("web wasn't started: Port 80 is already used by httpd.exe (PID 1234)."),
    ).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "start_project_services")?.args).toEqual({ projectId: "p1" });

    // The services can be changed later.
    await userEvent.click(within(table).getByRole("button", { name: "Choose services for Shop" }));
    const choose = screen.getByRole("form", { name: "Services for Shop" });
    await userEvent.click(within(choose).getByLabelText(/^web/));
    await userEvent.click(within(choose).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "set_project_services")?.args).toEqual({
        projectId: "p1",
        serviceIds: ["db"],
      }),
    );
    expect(screen.queryByRole("form", { name: "Services for Shop" })).not.toBeInTheDocument();
  });

  it("offers the site for a new project with its domain filled in", async () => {
    const added = project({ id: "p2", name: "Blog", rootPath: "C:\\Devluma\\www\\blog" });
    let projects: unknown[] = [];
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_preferences: () => preferencesView({ domainSuffix: "local" }),
      list_projects: () => projects,
      list_services: () => [
        {
          id: "w1",
          packageInstallationId: "i-ng",
          instanceName: "web",
          serviceType: "web_server",
          lifecycleAdapter: "nginx",
          desiredState: "running",
          observedState: "running",
          port: 80,
          bindAddress: "127.0.0.1",
          configPath: null,
          dataPath: null,
          pid: 1,
          processStartedAt: null,
          healthStatus: "healthy",
          lastExitCode: null,
          lastErrorCode: null,
          jobName: null,
          adminPort: null,
          updatedAt: "2026-10-12T10:00:00.000Z",
          role: "web_server",
          packageId: "nginx",
          packageName: "Nginx",
          version: "1.30.5",
          portConflicts: [],
        },
      ],
      create_project: () => {
        projects = [added];
        return { ...added, runtimes: undefined, detection: undefined };
      },
    });
    const { container } = await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add project" }))[0]!);
    const form = screen.getByRole("form", { name: "Add project" });
    await userEvent.type(within(form).getByLabelText("Name"), "Blog");
    await waitFor(() => expect(within(form).getByLabelText("Domain")).toHaveValue("blog.local"));
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));

    const site = await screen.findByRole("form", { name: "Add site" });
    expect(screen.getByText(/Blog was added/)).toBeInTheDocument();
    expect(within(site).getByLabelText("Domain")).toHaveValue("blog.local");
    expect(within(site).getByLabelText("Project")).toHaveValue("p2");
    // Nothing is applied until the user previews and applies.
    expect(mock.calls.some((c) => c.command === "apply_site" || c.command === "preview_site")).toBe(false);
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(site).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "Add site" })).not.toBeInTheDocument();
  });

  it("lists projects with their effective PHP and passes a11y checks", async () => {
    const { container } = await renderApp(
      "/projects",
      createMockAdapter({ ...defaultHandlers, list_projects: () => [project()] }),
    );
    const table = await screen.findByRole("table");
    expect(within(table).getByText("8.3.35 (default)")).toBeInTheDocument();
    expect(within(table).getByText("D:\\code\\shop")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("opens a project's website from its row", async () => {
    const site = {
      id: "s1",
      projectId: "p1",
      domain: "shop.test",
      documentRoot: String.raw`D:\code\shop\public`,
      webServerServiceId: "w1",
      phpInstallationId: null,
      httpPort: 80,
      httpsPort: null,
      sslEnabled: false,
      configPath: String.raw`C:\Devluma\services\w1\sites\s1.conf`,
      configHash: null,
      status: "active",
      hostsManaged: true,
      createdAt: "2026-10-11T10:00:00.000Z",
      updatedAt: "2026-10-11T10:00:00.000Z",
      projectName: "Shop",
      webServerName: "web",
      webServerRunning: true,
      url: "http://shop.test/",
      httpsUrl: null,
      phpVersion: null,
      phpOutdated: false,
      configPresent: true,
    };
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project(), project({ id: "p2", name: "Blog" })],
      list_sites: () => [site],
      open_site: () => "http://shop.test/",
    });
    await renderApp("/projects", mock);
    const table = await screen.findByRole("table");
    // Only the project with a site gets the button.
    expect(await within(table).findAllByRole("button", { name: "Open shop.test" })).toHaveLength(1);
    await userEvent.click(within(table).getByRole("button", { name: "Open shop.test" }));
    expect(await within(table).findByText("Opened http://shop.test/ in your browser.")).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "open_site")?.args).toEqual({ siteId: "s1" });
  });

  it("guides Python projects to a virtual environment", async () => {
    const python = (virtualEnvScripts: string | null) =>
      project({
        detection: {
          kind: "python",
          label: "Python",
          suggestedDocumentRoot: "D:\\code\\shop",
          virtualEnvScripts,
        },
      });
    const { unmount } = await renderApp(
      "/projects",
      createMockAdapter({ ...defaultHandlers, list_projects: () => [python(null)] }),
    );
    const table = await screen.findByRole("table");
    expect(table).toHaveTextContent("No virtual environment yet");
    expect(table).toHaveTextContent("python -m venv .venv");
    unmount();

    await renderApp(
      "/projects",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [python(String.raw`D:\code\shop\.venv\Scripts`)],
      }),
    );
    expect(await screen.findByRole("table")).toHaveTextContent("the project terminal uses it first");
  });

  it("previews and applies a project PHP pin", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project()],
      list_packages: () => catalog,
      preview_runtime_switch: (args) =>
        (args?.request as { installationId?: string }).installationId === "i84"
          ? preview()
          : preview({ noOp: true, changes: [] }),
      apply_runtime_switch: () => ({
        operationId: "op-1",
        operationType: "switch_runtime",
        status: "succeeded",
        startedAt: "2026-10-11T10:00:00.000Z",
      }),
    });
    await renderApp("/projects", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Change PHP for Shop" }));

    const dialog = await screen.findByRole("dialog", { name: "PHP for Shop" });
    expect(await within(dialog).findByText(/Nothing would change/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Apply" })).toBeDisabled();

    await userEvent.selectOptions(
      await within(dialog).findByLabelText("PHP version for this project"),
      "i84",
    );
    expect(await within(dialog).findByText("Shop: PHP 8.3.35 → PHP 8.4.26.")).toBeInTheDocument();
    expect(within(dialog).getByRole("list", { name: "Not affected" })).toHaveTextContent("PATH");
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(mock.calls.find((c) => c.command === "apply_runtime_switch")?.args).toEqual({
      request: { packageId: "php", scope: "project", projectId: "p1", installationId: "i84" },
    });
  });

  it("keeps the dialog open with the reason when verification fails", async () => {
    await renderApp(
      "/projects",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project()],
        list_packages: () => catalog,
        preview_runtime_switch: () => preview(),
        apply_runtime_switch: () => ({
          operationId: "op-2",
          operationType: "switch_runtime",
          status: "failed",
          errorCode: "INTEGRITY_ERROR",
          safeMessage: "Could not run php.exe: the system cannot find vcruntime140.dll",
          startedAt: "2026-10-11T10:00:00.000Z",
        }),
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Change PHP for Shop" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(
      await within(dialog).findByLabelText("PHP version for this project"),
      "i84",
    );
    await userEvent.click(await within(dialog).findByRole("button", { name: "Apply" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("vcruntime140.dll");
  });

  it("removes a project only after explaining the folder is kept", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project()],
      remove_project: () => null,
    });
    await renderApp("/projects", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Remove Shop from Devluma" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("The folder and every file in it stay exactly where they are.");
    expect(dialog).toHaveTextContent("D:\\code\\shop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove from Devluma" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "remove_project")).toBe(true));
  });

  it("opens a project terminal and says what's on PATH", async () => {
    await renderApp(
      "/projects",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project()],
        open_project_terminal: () => ({
          program: "cmder",
          pathPrefix: ["C:\\Devluma\\packages\\php\\8.4.26-x64"],
        }),
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal for Shop" }));
    expect(
      await screen.findByText(/Opened Cmder with .*8\.4\.26-x64 first on PATH \(that window only\)/),
    ).toBeInTheDocument();
  });

  it("opens a terminal outside projects with the default runtimes", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      open_terminal: () => ({ program: "power_shell", pathPrefix: [] }),
    });
    await renderApp("/projects", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }));
    expect(
      await screen.findByText("Opened Windows PowerShell. No runtime is selected yet."),
    ).toBeInTheDocument();
    expect(mock.calls.some((c) => c.command === "open_terminal")).toBe(true);
  });
});

describe("global default and PHP details", () => {
  it("makes an installed version the default through a preview", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_packages: () => catalog,
      preview_runtime_switch: () =>
        preview({
          scope: "global",
          projectName: undefined,
          changes: [
            "Default PHP: PHP 8.3.35 → PHP 8.4.26.",
            "Projects without their own PHP version will use PHP 8.4.26.",
          ],
          unchanged: ["Projects that pin their own PHP version keep it."],
        }),
      apply_runtime_switch: () => ({
        operationId: "op-3",
        operationType: "switch_runtime",
        status: "succeeded",
        startedAt: "2026-10-11T10:00:00.000Z",
      }),
    });
    await renderApp("/runtimes/php", mock);
    expect(await screen.findByText("Installed (default)")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Make PHP 8.3.35 the default" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Make PHP 8.4.26 the default" }));
    const dialog = await screen.findByRole("dialog", { name: "Make PHP 8.4.26 the default?" });
    expect(
      await within(dialog).findByText("Projects that pin their own PHP version keep it."),
    ).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Make default" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("shows PHP details and creates php.ini from a template", async () => {
    const details = {
      installPath: "C:\\Devluma\\packages\\php\\8.4.26-x64",
      iniPath: "C:\\Devluma\\packages\\php\\8.4.26-x64\\php.ini",
      iniExists: false,
      availableTemplates: ["development", "production"],
      extensionDir: "C:\\Devluma\\packages\\php\\8.4.26-x64\\ext",
      loadedModules: ["Core", "date"],
      availableExtensions: ["curl", "mbstring"],
      startupWarnings: [],
    };
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_php_details: () => details,
      create_php_ini: () => ({ ...details, iniExists: true, loadedModules: ["Core", "curl", "date"] }),
    });
    const { container } = await renderApp("/runtimes/php/installations/i84", mock);
    expect(await screen.findByText(/No php.ini yet/)).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(screen.getByRole("button", { name: "Create from development template" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("extension_dir");
    expect(dialog).toHaveTextContent("never overwritten");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create php.ini" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(mock.calls.find((c) => c.command === "create_php_ini")?.args).toEqual({
      installationId: "i84",
      template: "development",
    });
    expect(screen.getByText(/This version uses the php.ini above/)).toBeInTheDocument();
  });

  it("surfaces PHP startup warnings", async () => {
    await renderApp(
      "/runtimes/php/installations/i84",
      createMockAdapter({
        ...defaultHandlers,
        get_php_details: () => ({
          installPath: "C:\\p",
          iniPath: "C:\\p\\php.ini",
          iniExists: true,
          availableTemplates: [],
          extensionDir: "C:\\p\\ext",
          loadedModules: ["Core"],
          availableExtensions: ["curl"],
          startupWarnings: ["PHP Startup: Unable to load dynamic library 'curl'"],
        }),
      }),
    );
    expect(await screen.findByRole("region", { name: "PHP startup warnings" })).toHaveTextContent(
      "Unable to load dynamic library 'curl'",
    );
  });
});
