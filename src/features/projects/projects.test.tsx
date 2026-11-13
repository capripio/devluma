import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  catalogView,
  createMockAdapter,
  defaultHandlers,
  chooseFromMenu,
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
        return { project: { ...project(), runtimes: undefined }, siteOperationId: null, siteError: null };
      },
    });
    await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add project" }))[0]!);
    const form = screen.getByRole("form", { name: "Add project" });
    await userEvent.type(within(form).getByLabelText("Name"), "My Shop");
    // The folder follows the name and shows where it ends up.
    expect(within(form).getByLabelText("Folder")).toHaveValue("my-shop");
    expect(within(form).getByText(/Devluma\\www\\my-shop/)).toBeInTheDocument();
    // No web server yet: no site is offered, and the form says where to add one.
    expect(within(form).getByText(/No web server yet/)).toBeInTheDocument();
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
      site: null,
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
      create_project: () => ({
        project: { ...project(), runtimes: undefined },
        siteOperationId: null,
        siteError: null,
      }),
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
    await userEvent.click(within(form).getByLabelText("Create a website at"));
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "create_project")?.args).toEqual({
        name: "Shop",
        folder: "shop",
        serviceIds: ["web"],
        site: null,
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
    const { container } = await renderApp("/projects/p1", mock);
    // The taken port is shown before anything is started.
    expect(await screen.findByText("Port 80 is in use by httpd.exe (PID 1234).")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(screen.getByRole("button", { name: "Start all services for Shop" }));
    expect(await screen.findByText("Starting db.")).toBeInTheDocument();
    expect(
      screen.getByText("web wasn't started: Port 80 is already used by httpd.exe (PID 1234)."),
    ).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "start_project_services")?.args).toEqual({ projectId: "p1" });

    // The services can be changed later.
    await userEvent.click(screen.getByRole("button", { name: "Choose services for Shop" }));
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

  it("creates the site together with a new project", async () => {
    const added = project({ id: "p2", name: "Blog", rootPath: String.raw`C:\Devluma\www\blog` });
    let projects: unknown[] = [];
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_preferences: () => preferencesView({ domainSuffix: "local" }),
      list_projects: () => projects,
      list_services: () => [svc("w1", "web_server", { instanceName: "web", observedState: "running" })],
      create_project: () => {
        projects = [added];
        return {
          project: { ...added, runtimes: undefined, detection: undefined, serviceIds: undefined },
          siteOperationId: "op-site",
          siteError: null,
        };
      },
    });
    const { container } = await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add project" }))[0]!);
    const form = screen.getByRole("form", { name: "Add project" });
    await userEvent.type(within(form).getByLabelText("Name"), "Blog");
    await waitFor(() => expect(within(form).getByLabelText("Domain")).toHaveValue("blog.local"));
    expect(within(form).getByText(/on web \(port 80\)/)).toBeInTheDocument();
    expect(within(form).getByText(/Windows may ask/)).toBeInTheDocument();
    await userEvent.click(within(form).getByLabelText(/Also serve over HTTPS/));
    expect(await axeViolations(container)).toEqual([]);
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));

    expect(await screen.findByText("Creating blog.local…")).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "create_project")?.args).toEqual({
      name: "Blog",
      folder: "blog",
      serviceIds: ["w1"],
      site: { domain: "blog.local", webServerServiceId: "w1", https: true },
    });
    // No second review step: the site isn't handed to the site form.
    expect(screen.queryByRole("form", { name: "Add site" })).not.toBeInTheDocument();
  });

  it("installs WordPress from the web installer with a chosen version, its database and progress", async () => {
    const added = project({ id: "p9", name: "News", rootPath: String.raw`C:\Devluma\www\news` });
    let projects: unknown[] = [];
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => projects,
      list_services: () => [
        svc("w1", "web_server", { instanceName: "web" }),
        svc("m1", "database", {
          instanceName: "mariadb",
          role: "database",
          packageId: "mariadb",
          packageName: "MariaDB",
          port: 3306,
          observedState: "stopped",
        }),
      ],
      list_app_versions: (args) =>
        args?.app === "wordpress"
          ? {
              app: "wordpress",
              versions: [
                { value: "7.1.4", label: "WordPress 7.1.4", latest: "7.1.4", minPhp: "7.4", supported: true },
                { value: "7.0.8", label: "WordPress 7.0.8", latest: "7.0.8", minPhp: "7.4", supported: true },
              ],
              source: "Devluma's catalog (each release checked against its SHA-256)",
              unavailable: null,
            }
          : {
              app: "laravel",
              versions: [
                { value: "13.*", label: "Laravel 13.x", latest: "13.11.0", minPhp: "8.3", supported: false },
                { value: "12.*", label: "Laravel 12.x", latest: "12.9.0", minPhp: "8.2", supported: true },
              ],
              source: "packagist.org",
              unavailable: null,
            },
      preview_app_project: (args) => {
        const req = args?.request as { folder: string; version: string };
        return {
          app: "wordpress",
          label: "WordPress",
          targetPath: String.raw`C:\Devluma\www\\` + req.folder,
          phpVersion: "8.4.27",
          downloads: [`WordPress ${req.version} (30 MB)`],
          enableExtensions: ["mysqli"],
          databaseName: "news",
          steps: [
            `Download WordPress ${req.version} (30 MB) from wordpress.org, check its SHA-256 and unpack it into the project folder only.`,
            "Create the database news and write wp-config.php.",
          ],
          blocked: null,
        };
      },
      create_app_project: () => ({ operationId: "op-app", projectId: "p9" }),
    });
    const { container } = await renderApp("/projects", mock);
    // A separate action from "Add project", which only adds a folder.
    await userEvent.click((await screen.findAllByRole("button", { name: "Install web app" }))[0]!);
    const form = screen.getByRole("form", { name: "Install a web app" });
    expect(screen.queryByRole("form", { name: "Add project" })).not.toBeInTheDocument();

    // Versions: newest first; Laravel lines this PHP can't run are disabled, and the
    // newest supported one is preselected.
    await userEvent.click(within(form).getByRole("radio", { name: "Laravel" }));
    const laravel = await within(form).findByLabelText("Version");
    expect(laravel).toHaveValue("12.*");
    expect(
      within(laravel).getByRole("option", { name: /Laravel 13\.x \(13\.11\.0\) — needs PHP 8\.3/ }),
    ).toBeDisabled();
    expect(within(form).getByText(/Releases from packagist\.org/)).toBeInTheDocument();

    await userEvent.click(within(form).getByRole("radio", { name: "WordPress" }));
    const version = await within(form).findByLabelText("Version");
    expect(version).toHaveValue("7.1.4");
    await userEvent.selectOptions(version, "7.0.8");
    await userEvent.type(within(form).getByLabelText("Name"), "News");
    expect(within(form).getByLabelText("Database")).toHaveValue("m1");
    // A stopped database is started by the installer, not a reason to stop and ask.
    expect(
      within(form).getByRole("option", { name: "Create one on mariadb (port 3306, started for you)" }),
    ).toBeInTheDocument();
    expect(await within(form).findByText(/Download WordPress 7\.0\.8/)).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);
    await userEvent.click(within(form).getByRole("button", { name: "Install WordPress" }));

    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "create_app_project")?.args).toEqual({
        request: {
          name: "News",
          folder: "news",
          app: "wordpress",
          serviceIds: ["w1", "m1"],
          site: { domain: "news.test", webServerServiceId: "w1", https: false },
          databaseServiceId: "m1",
          version: "7.0.8",
        },
      }),
    );
    const progress = {
      operationId: "op-app",
      operationType: "create_app_project",
      targetId: "p9",
      startedAt: "2026-11-13T10:00:00.000Z",
    };
    act(() =>
      mock.emit("operation://progress", {
        ...progress,
        status: "running",
        phase: "Downloading WordPress 7.0.8",
      }),
    );
    expect(await within(form).findByText("Downloading WordPress 7.0.8")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Cancel installing the app" })).toBeInTheDocument();
    projects = [added];
    act(() => mock.emit("operation://progress", { ...progress, status: "succeeded" }));
    expect(await screen.findByRole("heading", { name: "News" })).toBeInTheDocument();
  });

  it("explains why an app can't be installed and keeps the button disabled", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_app_versions: () => ({
        app: "laravel",
        versions: [],
        source: "packagist.org",
        unavailable: "Couldn't read the releases from packagist.org (timed out).",
      }),
      preview_app_project: () => ({
        app: "laravel",
        label: "Laravel",
        targetPath: "",
        phpVersion: null,
        downloads: [],
        enableExtensions: [],
        databaseName: null,
        steps: [],
        blocked:
          "Laravel needs PHP. Install a PHP version and make it the default on the Runtimes & Packages page.",
      }),
    });
    await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Install web app" }))[0]!);
    const form = screen.getByRole("form", { name: "Install a web app" });
    await userEvent.click(within(form).getByRole("radio", { name: "Laravel" }));
    // No list: said plainly, and the newest compatible release is used.
    expect(await within(form).findByText(/Couldn't read the releases/)).toBeInTheDocument();
    await userEvent.type(within(form).getByLabelText("Name"), "Api");
    expect(await within(form).findByRole("alert")).toHaveTextContent("Laravel needs PHP");
    expect(within(form).getByRole("button", { name: "Install Laravel" })).toBeDisabled();
  });

  it("offers to install a web app into a project whose folder is empty", async () => {
    const empty = project({
      detection: {
        kind: null,
        label: "Unknown",
        suggestedDocumentRoot: String.raw`D:\code\shop`,
        virtualEnvScripts: null,
        empty: true,
      },
    });
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [empty],
      list_app_versions: () => ({
        app: "wordpress",
        versions: [
          { value: "7.1.4", label: "WordPress 7.1.4", latest: "7.1.4", minPhp: "7.4", supported: true },
        ],
        source: "Devluma's catalog",
        unavailable: null,
      }),
      preview_app_project: () => ({
        app: "wordpress",
        label: "WordPress",
        targetPath: String.raw`D:\code\shop`,
        phpVersion: "8.3.35",
        downloads: [],
        enableExtensions: [],
        databaseName: null,
        steps: ["Install it into Shop (D:\\code\\shop)."],
        blocked: null,
      }),
      create_app_project: () => ({ operationId: "op-x", projectId: "p1" }),
    });
    await renderApp("/projects/p1", mock);
    await userEvent.click(
      await screen.findByRole("button", { name: "Install WordPress, Laravel or Symfony here" }),
    );
    const form = screen.getByRole("form", { name: "Install a web app" });
    // The project is fixed: no name or folder to enter.
    expect(within(form).queryByLabelText("Name")).not.toBeInTheDocument();
    expect(within(form).getByText(/which is empty/)).toBeInTheDocument();
    // Enabled once the plan is back.
    await waitFor(() =>
      expect(within(form).getByRole("button", { name: "Install WordPress" })).toBeEnabled(),
    );
    await userEvent.click(within(form).getByRole("button", { name: "Install WordPress" }));
    await waitFor(() =>
      expect(
        (
          mock.calls.find((c) => c.command === "create_app_project")?.args as {
            request: Record<string, unknown>;
          }
        ).request,
      ).toMatchObject({ projectId: "p1", app: "wordpress", version: "7.1.4", serviceIds: [] }),
    );
  });

  it("runs Composer for a project and turns on missing PHP extensions first", async () => {
    let missing = ["openssl", "zip"];
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project()],
      get_composer_status: () => ({
        composerVersion: "2.11.2",
        phpVersion: "8.3.35",
        phpInstallationId: "i83",
        missingExtensions: missing,
        hasComposerJson: true,
        hasVendor: false,
        running: false,
      }),
      enable_composer_extensions: () => {
        missing = [];
        return null;
      },
      run_composer: () => "op-c",
    });
    await renderApp("/projects/p1", mock);
    expect(await screen.findByText(/doesn't load openssl, zip/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "composer install" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Turn them on" }));
    expect(await screen.findByText(/Turned on openssl, zip/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "composer install" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "composer install" }));
    expect(mock.calls.find((c) => c.command === "run_composer")?.args).toEqual({
      projectId: "p1",
      action: "install",
    });
  });

  it("lists projects calmly, then shows one project's details", async () => {
    const { container } = await renderApp(
      "/projects",
      createMockAdapter({ ...defaultHandlers, list_projects: () => [project()] }),
    );
    const table = await screen.findByRole("table");
    expect(within(table).getByText("8.3.35")).toBeInTheDocument();
    // No action buttons in the overview: actions live with the project.
    expect(within(table).queryAllByRole("button")).toEqual([]);
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(table).getByRole("link", { name: "Shop" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Shop" })).toBeInTheDocument();
    expect(screen.getByText("D:\\code\\shop")).toBeInTheDocument();
    expect(screen.getByText("(global default)")).toBeInTheDocument();
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
      embeddedPhp: null,
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
    // The domain itself opens the site; only the project with a site has one.
    expect(await within(table).findAllByRole("button", { name: "Open shop.test" })).toHaveLength(1);
    await userEvent.click(within(table).getByRole("button", { name: "Open shop.test" }));
    expect(await screen.findByText("Opened http://shop.test/ in your browser.")).toBeInTheDocument();
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
      "/projects/p1",
      createMockAdapter({ ...defaultHandlers, list_projects: () => [python(null)] }),
    );
    expect(await screen.findByText(/No virtual environment yet/)).toHaveTextContent("python -m venv .venv");
    unmount();

    await renderApp(
      "/projects/p1",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [python(String.raw`D:\code\shop\.venv\Scripts`)],
      }),
    );
    expect(await screen.findByText(/the project terminal uses it first/)).toBeInTheDocument();
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
    await renderApp("/projects/p1", mock);
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
      "/projects/p1",
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
    await renderApp("/projects/p1", mock);
    await chooseFromMenu("More actions for Shop", "Remove from Devluma");
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("The folder and every file in it stay exactly where they are.");
    expect(dialog).toHaveTextContent("D:\\code\\shop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove from Devluma" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "remove_project")).toBe(true));
  });

  it("opens the project folder in File Explorer", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project()],
      open_project_folder: () => null,
    });
    await renderApp("/projects/p1", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Open folder of Shop" }));
    expect(await screen.findByText("Opened D:\\code\\shop in File Explorer.")).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "open_project_folder")?.args).toEqual({ projectId: "p1" });
  });

  it("opens a project terminal and says what's on PATH", async () => {
    await renderApp(
      "/projects/p1",
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
      get_config: () => ({
        settings: [],
        settingsFile: "main",
        settingsUnavailable: null,
        applies: "restart",
        extensions: [{ name: "curl", enabled: true }],
        files: [],
      }),
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
    // The new php.ini opens in the settings panel.
    expect(await screen.findByRole("tab", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "curl" })).toBeChecked();
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
