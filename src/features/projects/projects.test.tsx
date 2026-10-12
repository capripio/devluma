import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  catalogView,
  createMockAdapter,
  defaultHandlers,
  packageView,
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

  it("adds a project and shows backend validation errors inline", async () => {
    let attempts = 0;
    const mock = createMockAdapter({
      ...defaultHandlers,
      create_project: () => {
        attempts += 1;
        if (attempts === 1) {
          throw {
            code: "VALIDATION_ERROR",
            message: "That folder doesn't exist or can't be opened.",
            recoverable: false,
          };
        }
        return { ...project(), runtimes: undefined };
      },
    });
    await renderApp("/projects", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add project" }))[0]!);
    const form = screen.getByRole("form", { name: "Add project" });
    await userEvent.type(within(form).getByLabelText("Name"), "Shop");
    await userEvent.type(within(form).getByLabelText("Folder"), "D:\\nope");
    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("doesn't exist");

    await userEvent.click(within(form).getByRole("button", { name: "Add project" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Add project" })).not.toBeInTheDocument());
    expect(mock.calls.filter((c) => c.command === "create_project").at(-1)?.args).toEqual({
      name: "Shop",
      rootPath: "D:\\nope",
    });
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
        open_project_terminal: () => ({ pathPrefix: ["C:\\Devluma\\packages\\php\\8.4.26-x64"] }),
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal for Shop" }));
    expect(await screen.findByText(/8\.4\.26-x64 first on PATH \(that window only\)/)).toBeInTheDocument();
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
