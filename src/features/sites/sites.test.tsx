import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  createMockAdapter,
  defaultHandlers,
  preferencesView,
  renderApp,
  chooseFromMenu,
} from "../../test/utils";

const project = {
  id: "p1",
  name: "My Shop",
  rootPath: String.raw`D:\code\shop`,
  projectType: "laravel",
  localDomain: null,
  notes: null,
  isFavorite: false,
  createdAt: "2026-10-12T10:00:00.000Z",
  updatedAt: "2026-10-12T10:00:00.000Z",
  lastOpenedAt: null,
  serviceIds: [],
  startServiceIds: [],
  detection: {
    kind: "laravel",
    label: "Laravel",
    suggestedDocumentRoot: String.raw`D:\code\shop\public`,
    virtualEnvScripts: null,
  },
  runtimes: [],
};

const nginx = {
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
};

const plan = {
  siteId: "site-1",
  domain: { normalized: "my-shop.test", warnings: [] },
  url: "http://my-shop.test/",
  documentRoot: String.raw`D:\code\shop\public`,
  configPath: String.raw`C:\Devluma\services\w1\sites\site-1.conf`,
  configText: "server { }",
  configDiff: "--- a\n+++ b\n+server_name my-shop.test;\n",
  php: { installationId: "i-php", version: "8.4.26", port: 9000, embedded: false },
  https: null,
  hosts: {
    path: String.raw`C:\Windows\System32\drivers\etc\hosts`,
    sha256Before: "abc",
    entries: [{ ip: "127.0.0.1", domain: "my-shop.test" }],
    changes: [{ domain: "my-shop.test", ipAddress: "127.0.0.1", action: "add" }],
    diff: "--- hosts\n+++ hosts\n+127.0.0.1 my-shop.test\n",
    needsElevation: true,
    unchanged: false,
  },
  webServerRunning: true,
  warnings: [],
};

const site = {
  id: "site-1",
  projectId: "p1",
  domain: "my-shop.test",
  documentRoot: String.raw`D:\code\shop\public`,
  webServerServiceId: "w1",
  phpInstallationId: "i-php",
  httpPort: 80,
  httpsPort: null,
  sslEnabled: false,
  configPath: String.raw`C:\Devluma\services\w1\sites\site-1.conf`,
  configHash: "h",
  status: "active",
  hostsManaged: true,
  createdAt: "2026-10-12T10:00:00.000Z",
  updatedAt: "2026-10-12T10:00:00.000Z",
  projectName: "My Shop",
  webServerName: "web",
  webServerRunning: true,
  url: "http://my-shop.test/",
  httpsUrl: null,
  phpVersion: "8.4.26",
  embeddedPhp: null,
  phpOutdated: false,
  configPresent: true,
};

describe("sites", () => {
  it("explains missing prerequisites", async () => {
    await renderApp("/sites");
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    expect(await screen.findByText(/Add a project first/)).toBeInTheDocument();
    expect(
      screen.getByText(/Create a web server service \(Nginx, Apache or Caddy\) first/),
    ).toBeInTheDocument();
  });

  it("says when PHP comes from FrankenPHP itself", async () => {
    const fp = {
      ...nginx,
      id: "w4",
      instanceName: "frankenphp",
      lifecycleAdapter: "frankenphp",
      packageId: "frankenphp",
      packageName: "FrankenPHP",
      adminPort: 2019,
    };
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [fp],
        preview_site: () => ({
          ...plan,
          php: { installationId: "i-fp", version: "8.5.12", port: 0, embedded: true },
          warnings: [
            "frankenphp serves PHP 8.5.12 itself; My Shop's PHP version pin doesn't apply to this site.",
          ],
        }),
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    expect(within(form).getByLabelText("Serve PHP with FrankenPHP's built-in PHP")).toBeChecked();
    await userEvent.click(within(form).getByRole("button", { name: "Preview changes" }));
    const preview = await within(form).findByRole("region", { name: "Site preview" });
    expect(preview).toHaveTextContent("PHP 8.5.12 (built into FrankenPHP)");
    expect(preview).not.toHaveTextContent("FastCGI");
    expect(preview).toHaveTextContent("PHP version pin doesn't apply");
  });

  it("offers every web server, not databases", async () => {
    const apache = {
      ...nginx,
      id: "w2",
      instanceName: "apache",
      lifecycleAdapter: "apache",
      packageId: "apache",
      packageName: "Apache httpd",
      port: 8081,
    };
    const caddy = {
      ...nginx,
      id: "w3",
      instanceName: "caddy",
      lifecycleAdapter: "caddy",
      packageId: "caddy",
      packageName: "Caddy",
      port: 8082,
      adminPort: 2019,
    };
    const db = {
      ...nginx,
      id: "d1",
      instanceName: "db",
      lifecycleAdapter: "mariadb",
      role: "database",
      packageName: "MariaDB",
      port: 3306,
    };
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [nginx, apache, caddy, db],
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    const options = within(within(form).getByLabelText("Web server")).getAllByRole("option");
    expect(options.map((o) => o.textContent)).toEqual([
      "web — Nginx (port 80)",
      "apache — Apache httpd (port 8081)",
      "caddy — Caddy (port 8082)",
    ]);
  });

  it("suggests domains with the suffix chosen in Settings", async () => {
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [nginx],
        get_preferences: () => preferencesView({ domainSuffix: "local" }),
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    expect(within(form).getByLabelText("Domain")).toHaveValue("my-shop.local");
  });

  it("previews the exact changes, explains elevation, then applies what was previewed", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project],
      list_services: () => [nginx],
      preview_site: () => plan,
      apply_site: () => ({ operationId: "op-1" }),
    });
    const { container } = await renderApp("/sites", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    // Defaults come from detection: domain from the name, docroot from Laravel's public/.
    expect(within(form).getByLabelText("Domain")).toHaveValue("my-shop.test");
    expect(within(form).getByLabelText(/Document root/)).toHaveValue(String.raw`D:\code\shop\public`);
    expect(within(form).getByLabelText(/Serve PHP/)).toBeChecked();

    await userEvent.click(within(form).getByRole("button", { name: "Preview changes" }));
    const preview = await within(form).findByRole("region", { name: "Site preview" });
    expect(within(preview).getByLabelText("Site configuration changes")).toHaveTextContent(
      "server_name my-shop.test",
    );
    expect(within(preview).getByLabelText("Hosts file changes")).toHaveTextContent("+127.0.0.1 my-shop.test");
    expect(preview).toHaveTextContent("Windows will ask for administrator approval");
    expect(preview).toHaveTextContent("PHP 8.4.26 (FastCGI on 127.0.0.1:9000)");
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(form).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Add site" })).not.toBeInTheDocument());
    expect(mock.calls.find((c) => c.command === "apply_site")?.args).toEqual({
      request: {
        siteId: "site-1",
        projectId: "p1",
        domain: "my-shop.test",
        webServerServiceId: "w1",
        usePhp: true,
        documentRoot: String.raw`D:\code\shop\public`,
        manageHosts: true,
        https: false,
        expectedHostsSha256: "abc",
      },
    });
  });

  it("drops the preview when an input changes", async () => {
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [nginx],
        preview_site: () => plan,
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    await userEvent.click(within(form).getByRole("button", { name: "Preview changes" }));
    await within(form).findByRole("region", { name: "Site preview" });
    await userEvent.type(within(form).getByLabelText("Domain"), "x");
    expect(within(form).queryByRole("region", { name: "Site preview" })).not.toBeInTheDocument();
    expect(within(form).queryByRole("button", { name: "Apply" })).not.toBeInTheDocument();
  });

  it("shows preview errors inline", async () => {
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [nginx],
        preview_site: () => {
          throw {
            code: "VALIDATION_ERROR",
            message: "my-shop.test is already used by another site.",
            recoverable: true,
          };
        },
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    await userEvent.click(within(form).getByRole("button", { name: "Preview changes" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("already used by another site");
  });

  it("lists sites with actions, outdated PHP and check results", async () => {
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_sites: () => [{ ...site, phpOutdated: true }],
        check_site: () => ({
          url: "http://my-shop.test/",
          status: 200,
          message: "http://my-shop.test/ answered with HTTP 200.",
        }),
      }),
    );
    const table = await screen.findByRole("table");
    expect(table).toHaveTextContent("Project PHP changed — re-apply to switch");
    await chooseFromMenu("More actions for my-shop.test", "Check");
    expect(
      await within(table).findByText("http://my-shop.test/ answered with HTTP 200."),
    ).toBeInTheDocument();
  });

  it("explains removal: hosts entry, admin prompt, folder untouched", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_sites: () => [site],
      remove_site: () => ({ operationId: "op-2" }),
    });
    await renderApp("/sites", mock);
    await chooseFromMenu("More actions for my-shop.test", "Remove");
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("administrator approval");
    expect(dialog).toHaveTextContent("The project folder isn't touched");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove site" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "remove_site")).toBe(true));
  });

  it("previews HTTPS honestly when the local CA isn't trusted", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project],
      list_services: () => [nginx],
      preview_site: (args) => ({
        ...plan,
        https: (args?.request as { https?: boolean }).https
          ? {
              port: 443,
              url: "https://my-shop.test/",
              certificateId: null,
              caExists: false,
              caTrusted: false,
            }
          : null,
      }),
    });
    await renderApp("/sites", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    await userEvent.click(within(form).getByLabelText(/Also serve over HTTPS/));
    await userEvent.click(within(form).getByRole("button", { name: "Preview changes" }));
    const preview = await within(form).findByRole("region", { name: "Site preview" });
    expect(preview).toHaveTextContent("https://my-shop.test/");
    expect(preview).toHaveTextContent("a certificate is created by Devluma's local CA");
    expect(preview).toHaveTextContent("Browsers will show a warning until you trust Devluma's local CA");
    expect(preview).toHaveTextContent("Applying this site doesn't trust anything");
    expect(within(preview).getByRole("link", { name: "SSL Certificates" })).toHaveAttribute("href", "/ssl");
  });

  const localCa = (trustedNow: boolean) => ({
    id: "ca",
    commonName: "Devluma Local Development CA (PC)",
    sans: [],
    certificatePath: String.raw`C:\Devluma\certificates\ca\ca.pem`,
    privateKeyPath: String.raw`C:\Devluma\certificates\ca\ca.key.dpapi`,
    issuerType: "local_ca",
    fingerprintSha256: "ab".repeat(32),
    notBefore: "2026-10-11T00:00:00Z",
    notAfter: "2036-10-12T00:00:00Z",
    trustStore: trustedNow ? "CurrentUser\\Root" : null,
    trusted: trustedNow,
    createdAt: "2026-10-12T00:00:00Z",
    updatedAt: "2026-10-12T00:00:00Z",
    daysLeft: 3650,
    expiringSoon: false,
    expired: false,
    trustedNow,
    boundSites: [],
    keyProtection: "Encrypted with Windows DPAPI for your account",
  });

  it("warns as soon as HTTPS is ticked when this computer doesn't trust the local CA", async () => {
    const { container } = await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [nginx],
        list_certificates: () => [],
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    expect(within(form).queryByText("SSL isn't set up on this computer yet")).not.toBeInTheDocument();
    await userEvent.click(within(form).getByLabelText(/Also serve over HTTPS/));

    const notice = await within(form).findByRole("status");
    expect(notice).toHaveTextContent("SSL isn't set up on this computer yet");
    expect(notice).toHaveTextContent("hasn't been created and trusted yet");
    expect(notice).toHaveTextContent("https://my-shop.test is not secure");
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(notice).getByRole("link", { name: "Set up SSL Certificates" }));
    expect(await screen.findByRole("heading", { level: 1, name: "SSL Certificates" })).toBeInTheDocument();
  });

  it("says when the local CA exists but isn't trusted, and stays quiet once it is", async () => {
    let trusted = false;
    await renderApp(
      "/sites",
      createMockAdapter({
        ...defaultHandlers,
        list_projects: () => [project],
        list_services: () => [nginx],
        list_certificates: () => [localCa(trusted)],
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add site" });
    const https = within(form).getByLabelText(/Also serve over HTTPS/);
    await userEvent.click(https);
    expect(await within(form).findByRole("status")).toHaveTextContent(
      "exists, but this computer doesn't trust it",
    );

    // Checked again each time HTTPS is ticked.
    trusted = true;
    await userEvent.click(https);
    await userEvent.click(https);
    expect(
      await within(form).findByText(/This computer trusts Devluma's local certificate authority/),
    ).toBeInTheDocument();
    expect(within(form).queryByText("SSL isn't set up on this computer yet")).not.toBeInTheDocument();
  });
});
