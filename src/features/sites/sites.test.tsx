import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

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
};

const plan = {
  siteId: "site-1",
  domain: { normalized: "my-shop.test", warnings: [] },
  url: "http://my-shop.test/",
  documentRoot: String.raw`D:\code\shop\public`,
  configPath: String.raw`C:\Devluma\services\w1\sites\site-1.conf`,
  configText: "server { }",
  configDiff: "--- a\n+++ b\n+server_name my-shop.test;\n",
  php: { installationId: "i-php", version: "8.4.26", port: 9000 },
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
  phpOutdated: false,
  configPresent: true,
};

describe("sites", () => {
  it("explains missing prerequisites", async () => {
    await renderApp("/sites");
    await userEvent.click((await screen.findAllByRole("button", { name: "Add site" }))[0]!);
    expect(await screen.findByText(/Add a project first/)).toBeInTheDocument();
    expect(screen.getByText(/Create an Nginx service first/)).toBeInTheDocument();
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
    await userEvent.click(within(table).getByRole("button", { name: "Check my-shop.test" }));
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
    await userEvent.click(await screen.findByRole("button", { name: "Remove my-shop.test" }));
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
  });
});
