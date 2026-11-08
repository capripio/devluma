import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";
import { errorLine } from "./ConfigEditor";
import { summarize } from "./settingValues";

const service = {
  id: "s1",
  packageInstallationId: "i-ng",
  instanceName: "web",
  serviceType: "web_server",
  lifecycleAdapter: "nginx",
  desiredState: "running",
  observedState: "running",
  port: 8080,
  bindAddress: "127.0.0.1",
  configPath: "C:\\Devluma\\services\\s1\\conf\\nginx.conf",
  dataPath: null,
  pid: 4242,
  processStartedAt: null,
  healthStatus: "healthy",
  lastExitCode: null,
  lastErrorCode: null,
  jobName: null,
  adminPort: null,
  updatedAt: "2026-10-22T10:00:00.000Z",
  role: "web_server",
  packageId: "nginx",
  packageName: "Nginx",
  version: "1.30.5",
  portConflicts: [],
};

const setting = (overrides: Record<string, unknown>) => ({
  help: "Help text.",
  group: "Requests",
  default: null,
  defaultLabel: null,
  value: null,
  raw: null,
  ...overrides,
});

const view = (overrides: Record<string, unknown> = {}) => ({
  settings: [
    setting({
      key: "client_max_body_size",
      label: "Largest request",
      kind: { type: "size", min: 1, max: 65536, unlimited: "0" },
      default: "1",
    }),
    setting({
      key: "gzip",
      label: "Compress responses",
      group: "Performance",
      kind: { type: "toggle" },
      default: "off",
      value: "on",
    }),
  ],
  settingsFile: null,
  settingsUnavailable: null,
  applies: "reload",
  extensions: null,
  files: [
    {
      key: "custom",
      label: "custom.conf",
      path: "C:\\Devluma\\services\\s1\\conf\\custom.conf",
      editable: true,
      readOnlyReason: null,
      language: "nginx",
      note: "Your own directives.",
      text: "# Your own nginx http-level directives.\r\n",
      hash: "h1",
      backups: [{ id: "20261021T101010000Z.bak", createdAt: "2026-10-21T10:10:10Z" }],
    },
    {
      key: "main",
      label: "nginx.conf",
      path: "C:\\Devluma\\services\\s1\\conf\\nginx.conf",
      editable: false,
      readOnlyReason: null,
      language: "nginx",
      note: "Devluma writes this file before every start.",
      text: "worker_processes 1;\n",
      hash: "h2",
      backups: [],
    },
  ],
  ...overrides,
});

const preview = {
  path: "C:\\Devluma\\services\\s1\\conf\\nginx.conf",
  diff: "--- a\n+++ b\n@@ -1 +1,2 @@\n+  client_max_body_size 64m;\n",
  check: "nginx -t checks the whole configuration.",
  applies: "reload",
};

async function openConfig(handlers: Record<string, (args?: Record<string, unknown>) => unknown>) {
  const mock = createMockAdapter({
    ...defaultHandlers,
    list_services: () => [service],
    get_config: () => view(),
    preview_config_change: () => preview,
    ...handlers,
  });
  const r = await renderApp("/services/s1", mock);
  await userEvent.click(await screen.findByRole("tab", { name: "Configuration" }));
  await screen.findByRole("tab", { name: "Settings" });
  return r;
}

describe("service configuration", () => {
  it("previews simple settings, then saves them through the check", async () => {
    const { mock, container } = await openConfig({
      apply_config_change: () => ({
        message: "Saved and checked. web reloaded and uses it now.",
        warnings: [],
        restart: [],
        view: view(),
      }),
    });
    const form = screen.getByRole("form", { name: "Settings" });
    expect(within(form).getByText(/writes these into the configuration it generates/)).toBeInTheDocument();
    expect(within(form).getByLabelText("Compress responses")).toBeChecked();
    expect(await axeViolations(container)).toEqual([]);

    const size = within(form).getByLabelText("Largest request");
    await userEvent.clear(size);
    await userEvent.type(size, "64");
    expect(within(form).getByText("1 change")).toBeInTheDocument();
    await userEvent.click(within(form).getByRole("button", { name: "Review and save" }));

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Largest request: 1 MB → 64 MB");
    expect(await within(dialog).findByText(/client_max_body_size 64m/)).toBeInTheDocument();
    expect(dialog).toHaveTextContent("nginx -t checks the whole configuration.");
    expect(dialog).toHaveTextContent("puts them back if the check fails");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save and check" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    expect(mock.calls.find((c) => c.command === "apply_config_change")?.args).toEqual({
      target: { kind: "service", serviceId: "s1" },
      change: { kind: "settings", values: { client_max_body_size: "64" }, extensions: {} },
    });
    expect(screen.getByText("Saved and checked. web reloaded and uses it now.")).toBeInTheDocument();
  });

  it("blocks out-of-range values and can go back to the default", async () => {
    const { mock } = await openConfig({});
    const form = screen.getByRole("form", { name: "Settings" });
    const size = within(form).getByLabelText("Largest request");
    await userEvent.clear(size);
    await userEvent.type(size, "99999");
    expect(within(form).getByText("Enter 1–65,536 MB.")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Review and save" })).toBeDisabled();

    await userEvent.click(within(form).getByRole("button", { name: "Discard changes" }));
    // gzip is set (on); "Use default" turns it back to the tool's default.
    await userEvent.click(within(form).getByRole("button", { name: "Use default" }));
    expect(within(form).getByLabelText("Compress responses")).not.toBeChecked();
    await userEvent.click(within(form).getByRole("button", { name: "Review and save" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Compress responses: On → Off (default)");
    expect(mock.calls.find((c) => c.command === "preview_config_change")?.args?.change).toEqual({
      kind: "settings",
      values: { gzip: null },
      extensions: {},
    });
  });

  it("edits a file, keeps the dialog open with the tool's error and marks the line", async () => {
    const { mock } = await openConfig({
      apply_config_change: () => {
        throw {
          code: "CONFIG_INVALID",
          message:
            'nginx rejected the configuration: nginx: [emerg] invalid value "onn" in C:/Devluma/services/s1/conf/custom.conf:2 Nothing was changed.',
          recoverable: true,
        };
      },
    });
    await userEvent.click(screen.getByRole("tab", { name: "Advanced" }));
    const editor = await screen.findByLabelText("custom.conf");
    expect(editor).toHaveValue("# Your own nginx http-level directives.\n");
    expect(screen.getByRole("button", { name: "Review and save" })).toBeDisabled();

    fireEvent.change(editor, { target: { value: "# mine\ngzip onn;\n" } });
    await userEvent.click(screen.getByRole("button", { name: "Review and save" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Exact change");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save and check" }));
    expect(await within(dialog).findByText(/invalid value "onn"/)).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByLabelText("custom.conf")).toHaveAttribute("data-marked", "2");
    expect(mock.calls.find((c) => c.command === "apply_config_change")?.args?.change).toEqual({
      kind: "file",
      file: "custom",
      text: "# mine\ngzip onn;\n",
      baseHash: "h1",
    });

    // The generated file is read-only.
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await userEvent.click(screen.getByRole("radio", { name: /nginx\.conf/ }));
    expect(screen.getByLabelText("nginx.conf (read-only)")).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: "Review and save" })).not.toBeInTheDocument();
  });

  it("loads a previous version into the editor", async () => {
    const { mock } = await openConfig({ read_config_backup: () => "gzip on;\n" });
    await userEvent.click(screen.getByRole("tab", { name: "Advanced" }));
    await screen.findByLabelText("custom.conf");
    await userEvent.selectOptions(screen.getByLabelText("Previous versions"), "20261021T101010000Z.bak");
    await waitFor(() => expect(screen.getByLabelText("custom.conf")).toHaveValue("gzip on;\n"));
    expect(screen.getByText(/A previous version is loaded/)).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "read_config_backup")?.args).toEqual({
      target: { kind: "service", serviceId: "s1" },
      file: "custom",
      backupId: "20261021T101010000Z.bak",
    });
  });

  it("offers a restart when the running service keeps its old settings", async () => {
    const { mock } = await openConfig({
      apply_config_change: () => ({
        message: "Saved and checked. Restart web to use the new configuration.",
        warnings: ["option 'max_connections': unsigned value 99999999 adjusted to 100000"],
        restart: [{ serviceId: "s1", name: "web" }],
        view: view(),
      }),
      restart_service: () => ({ operationId: "op-r" }),
    });
    const form = screen.getByRole("form", { name: "Settings" });
    await userEvent.click(within(form).getByLabelText("Compress responses"));
    await userEvent.click(within(form).getByRole("button", { name: "Review and save" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Exact change");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save and check" }));
    expect(await screen.findByText(/adjusted to 100000/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Restart web now" }));
    expect(await screen.findByText(/Restarting web/)).toBeInTheDocument();
    expect(mock.calls.some((c) => c.command === "restart_service")).toBe(true);
  });

  it("sends PHP backends to their PHP version's page", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_services: () => [
        { ...service, role: "fast_cgi", lifecycleAdapter: "php", packageInstallationId: "i-php" },
      ],
    });
    await renderApp("/services/s1", mock);
    await userEvent.click(await screen.findByRole("tab", { name: "Configuration" }));
    expect(screen.getByRole("link", { name: /PHP 1\.30\.5 page/ })).toHaveAttribute(
      "href",
      "/runtimes/php/installations/i-php",
    );
    expect(mock.calls.some((c) => c.command === "get_config")).toBe(false);
  });
});

describe("PHP settings", () => {
  it("toggles extensions and refuses to edit a file holding a password", async () => {
    const details = {
      installPath: "C:\\Devluma\\bin\\php\\8.5.11",
      iniPath: "C:\\Devluma\\bin\\php\\8.5.11\\php.ini",
      iniExists: true,
      availableTemplates: ["development"],
      extensionDir: "C:\\Devluma\\bin\\php\\8.5.11\\ext",
      loadedModules: ["Core"],
      availableExtensions: ["curl", "mbstring"],
      startupWarnings: [],
    };
    const phpView = {
      settings: [
        setting({
          key: "memory_limit",
          label: "Memory limit",
          group: "Resources",
          kind: { type: "size", min: 16, max: 65536, unlimited: "-1" },
          default: "128",
          raw: "1500K",
        }),
      ],
      settingsFile: "main",
      settingsUnavailable: null,
      applies: "restart",
      extensions: [
        { name: "curl", enabled: false },
        { name: "mbstring", enabled: true },
      ],
      files: [
        {
          key: "main",
          label: "php.ini",
          path: details.iniPath,
          editable: true,
          readOnlyReason: "This file contains a password or token, which Devluma never shows.",
          language: "ini",
          note: "Shared by this PHP version.",
          text: "smtp_password = [REDACTED]\n",
          hash: "h",
          backups: [],
        },
      ],
    };
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_php_details: () => details,
      get_config: () => phpView,
      preview_config_change: () => ({ ...preview, check: "PHP starts with the new php.ini." }),
    });
    const { container } = await renderApp("/runtimes/php/installations/i85", mock);
    const form = await screen.findByRole("form", { name: "Settings" });
    expect(
      within(form).getByText(/Saved into php.ini, the same file the advanced editor shows/),
    ).toBeInTheDocument();
    expect(within(form).getByText("1500K")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(form).getByRole("checkbox", { name: "curl" }));
    await userEvent.click(within(form).getByRole("button", { name: "Review and save" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Extension curl: turn on");
    expect(dialog).toHaveTextContent("copied to Devluma's backups");
    expect(mock.calls.find((c) => c.command === "preview_config_change")?.args).toEqual({
      target: { kind: "php", installationId: "i85" },
      change: { kind: "settings", values: {}, extensions: { curl: true } },
    });
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    await userEvent.click(screen.getByRole("tab", { name: "Advanced" }));
    expect(await screen.findByText(/contains a password or token/)).toBeInTheDocument();
    expect(screen.getByLabelText("php.ini (read-only)")).toHaveAttribute("readonly");
  });
});

describe("helpers", () => {
  it("finds the line a tool's error points at", () => {
    expect(errorLine("in C:/x/custom.conf:12", "custom.conf")).toBe(12);
    expect(errorLine("AH00526: Syntax error on line 3 of C:/x/conf/custom.conf:", "custom.conf")).toBe(3);
    expect(errorLine("syntax error, unexpected end of file in C:\\x\\php.ini on line 4", "php.ini")).toBe(4);
    expect(
      errorLine('syntax error in file "C:/d/postgresql.conf" line 890, near token', "postgresql.conf"),
    ).toBe(890);
    expect(errorLine("unknown variable 'foo_bar=1'", "my.ini")).toBeNull();
    expect(errorLine("in C:/x/nginx.conf:4", "custom.conf")).toBeNull();
  });

  it("summarizes changes in plain words", () => {
    const s = setting({
      key: "memory_limit",
      label: "Memory limit",
      kind: { type: "size", min: 16, max: 65536, unlimited: "-1" },
      default: "128",
      value: "256",
    });
    expect(summarize([s] as never, { values: { memory_limit: "unlimited" }, extensions: {} })).toEqual([
      "Memory limit: 256 MB → No limit",
    ]);
    expect(summarize([s] as never, { values: { memory_limit: "2048" }, extensions: {} })).toEqual([
      "Memory limit: 256 MB → 2 GB",
    ]);
  });
});
