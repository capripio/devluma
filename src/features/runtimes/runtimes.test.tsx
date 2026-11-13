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
  renderApp,
  versionView,
} from "../../test/utils";

const php = packageView("php", [versionView("php", "8.3.12"), versionView("php", "8.4.1")], {
  displayName: "PHP",
});
const redis = packageView("redis", [versionView("redis", "7.4.0")], {
  displayName: "Redis",
  category: "cache",
});

function withCatalog(view: unknown, extra: Record<string, (args?: Record<string, unknown>) => unknown> = {}) {
  return createMockAdapter({ ...defaultHandlers, list_packages: () => view, ...extra });
}

describe("runtimes catalog", () => {
  it("adds pnpm to an installed Node.js and keeps WordPress from becoming a default", async () => {
    const installed = (id: string) => ({
      installation: {
        id,
        status: "installed",
        installPath: `D:\\Devluma\\bin\\node\\${id}`,
        isGlobalDefault: false,
        installedAt: "2026-11-13T10:00:00.000Z",
      },
    });
    const node = packageView("node", [versionView("node", "24.21.0", installed("n24"))], {
      displayName: "Node.js",
    });
    let enabled: string[] = [];
    const mock = withCatalog(catalogView([node]), {
      get_node_tools: () => ({ installationId: "n24", corepack: true, enabled }),
      set_node_tools: (args) => {
        enabled = args?.enabled as string[];
        return { installationId: "n24", corepack: true, enabled };
      },
    });
    const { container } = await renderApp("/runtimes/node", mock);
    const section = await screen.findByRole("region", { name: "Package managers" });
    const pnpm = await within(section).findByLabelText("pnpm");
    expect(pnpm).not.toBeChecked();
    expect(within(section).getByRole("button", { name: "Save" })).toBeDisabled();
    await userEvent.click(pnpm);
    await userEvent.click(within(section).getByRole("button", { name: "Save" }));
    expect(await within(section).findByText("Saved.")).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "set_node_tools")?.args).toEqual({
      installationId: "n24",
      enabled: ["pnpm"],
    });
    expect(await axeViolations(container)).toEqual([]);
  });

  it("keeps web apps out of the package list; they're installed into projects", async () => {
    const wp = packageView("wordpress", [versionView("wordpress", "7.1.4")], {
      displayName: "WordPress",
      category: "application",
    });
    await renderApp("/runtimes", withCatalog(catalogView([php, wp])));
    const table = await screen.findByRole("table");
    expect(within(table).getByRole("link", { name: "PHP" })).toBeInTheDocument();
    expect(within(table).queryByRole("link", { name: "WordPress" })).not.toBeInTheDocument();
    expect(
      within(screen.getByLabelText("Category")).queryByRole("option", { name: "Application" }),
    ).toBeNull();
  });

  it("doesn't offer a default for an application package", async () => {
    const wp = packageView(
      "wordpress",
      [
        versionView("wordpress", "7.1.4", {
          installation: {
            id: "wp1",
            status: "installed",
            installPath: String.raw`D:\Devluma\bin\wordpress\7.1.4-x64`,
            isGlobalDefault: false,
            installedAt: "2026-11-13T10:00:00.000Z",
          },
        }),
      ],
      { displayName: "WordPress", category: "application" },
    );
    await renderApp("/runtimes/wordpress", withCatalog(catalogView([wp])));
    expect(await screen.findByRole("button", { name: "Uninstall WordPress 7.1.4" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Make WordPress/ })).not.toBeInTheDocument();
    expect(screen.getByText("Application")).toBeInTheDocument();
  });
  it("says honestly when the catalog has no packages", async () => {
    await renderApp("/runtimes");
    expect(await screen.findByText("The package catalog is empty")).toBeInTheDocument();
  });

  it("lists packages and filters them", async () => {
    const { container } = await renderApp("/runtimes", withCatalog(catalogView([php, redis])));
    const table = await screen.findByRole("table");
    expect(within(table).getByRole("link", { name: "PHP" })).toHaveAttribute("href", "/runtimes/php");
    expect(within(table).getByRole("link", { name: "Redis" })).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.selectOptions(screen.getByLabelText("Category"), "cache");
    expect(within(screen.getByRole("table")).queryByRole("link", { name: "PHP" })).not.toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText("Category"), "all");
    await userEvent.type(screen.getByLabelText("Search"), "nope");
    expect(screen.getByText("No packages match these filters.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(screen.getByRole("link", { name: "PHP" })).toBeInTheDocument();
  });

  it("filters by install state", async () => {
    const installedPhp = packageView(
      "php",
      [
        versionView("php", "8.4.1", {
          installation: {
            id: "i1",
            status: "installed",
            installPath: "C:\\p",
            isGlobalDefault: false,
            installedAt: "2026-10-11T10:00:00.000Z",
          },
        }),
      ],
      { displayName: "PHP" },
    );
    await renderApp("/runtimes", withCatalog(catalogView([installedPhp, redis])));
    await screen.findByRole("table");
    await userEvent.selectOptions(screen.getByLabelText("Install state"), "installed");
    const table = screen.getByRole("table");
    expect(within(table).getByRole("link", { name: "PHP" })).toBeInTheDocument();
    expect(within(table).queryByRole("link", { name: "Redis" })).not.toBeInTheDocument();
    expect(within(table).getByText("8.4.1")).toBeInTheDocument();
  });

  it("shows rejected catalog entries", async () => {
    await renderApp(
      "/runtimes",
      withCatalog(
        catalogView([php], [{ packageId: "evil", message: 'Unknown adapter "shell"; package unavailable.' }]),
      ),
    );
    const issues = await screen.findByRole("region", { name: "Catalog problems" });
    expect(issues).toHaveTextContent("1 catalog entry was rejected");
    expect(issues).toHaveTextContent("evil: Unknown adapter");
  });
});

describe("package detail and install", () => {
  it("discloses source, size, checksum and target before installing", async () => {
    const mock = withCatalog(catalogView([php]), {
      install_package: () => ({ operationId: "op-9" }),
    });
    await renderApp("/runtimes/php", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Install PHP 8.4.1" }));

    const dialog = screen.getByRole("dialog", { name: "Install PHP 8.4.1?" });
    expect(dialog).toHaveTextContent("Downloads 30 MB from downloads.example.org");
    expect(dialog).toHaveTextContent("a".repeat(64));
    expect(dialog).toHaveTextContent("No administrator permission is needed");
    expect(dialog).toHaveTextContent("8.4.1-x64");
    expect(await axeViolations(document.body)).toEqual([]);

    await userEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(mock.calls.find((c) => c.command === "install_package")?.args).toEqual({
      request: { packageId: "php", version: "8.4.1", architecture: "x64" },
    });
  });

  it("keeps the dialog open with a clear error when preflight fails", async () => {
    await renderApp(
      "/runtimes/php",
      withCatalog(catalogView([php]), {
        install_package: () => {
          throw {
            code: "VALIDATION_ERROR",
            message: "Install Visual C++ Redistributable first.",
            recoverable: false,
          };
        },
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Install PHP 8.4.1" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Install Visual C++ Redistributable first.",
    );
  });

  it("shows live progress with cancel, then refreshes when the install finishes", async () => {
    let installed = false;
    const mock = withCatalog(undefined, {
      list_packages: () =>
        catalogView([
          packageView(
            "php",
            [
              versionView(
                "php",
                "8.4.1",
                installed
                  ? {
                      installation: {
                        id: "i1",
                        status: "installed",
                        installPath: "C:\\Devluma\\packages\\php\\8.4.1-x64",
                        isGlobalDefault: false,
                        installedAt: "2026-10-11T10:00:00.000Z",
                      },
                    }
                  : {},
              ),
            ],
            { displayName: "PHP" },
          ),
        ]),
      cancel_operation: () => null,
    });
    await renderApp("/runtimes/php", mock);
    await screen.findByRole("button", { name: "Install PHP 8.4.1" });

    const progress = operation({
      operationId: "op-7",
      targetId: "php@8.4.1@x64",
      phase: "Downloading",
      current: 10,
      total: 40,
    });
    act(() => mock.emit("operation://progress", progress));
    expect(screen.getByRole("progressbar", { name: "Download progress for PHP 8.4.1" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Cancel installing PHP 8.4.1" }));
    expect(mock.calls.find((c) => c.command === "cancel_operation")?.args).toEqual({ operationId: "op-7" });

    installed = true;
    act(() => mock.emit("operation://progress", { ...progress, status: "succeeded" }));
    expect(await screen.findByText("Installed")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Install PHP 8.4.1/ })).not.toBeInTheDocument();
  });

  it("offers retry after a failed attempt and explains unavailable versions", async () => {
    const view = catalogView([
      packageView(
        "php",
        [
          versionView("php", "8.4.1"),
          versionView("php", "8.4.2", {
            key: "php@8.4.2@arm64",
            architecture: "arm64",
            unavailableReason: "Built for arm64; this computer can't run it.",
          }),
        ],
        { displayName: "PHP" },
      ),
    ]);
    await renderApp(
      "/runtimes/php",
      withCatalog(view, {
        list_operations: () => [
          operation({
            targetId: "php@8.4.1@x64",
            status: "failed",
            safeMessage: "Checksum mismatch: expected SHA-256 aaa…, got bbb…",
          }),
        ],
      }),
    );
    expect(await screen.findByText(/Last attempt failed: Checksum mismatch/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry installing PHP 8.4.1" })).toBeEnabled();
    const arm = screen.getByRole("button", { name: "Install PHP 8.4.2" });
    expect(arm).toBeDisabled();
    expect(arm).toHaveAccessibleDescription("Built for arm64; this computer can't run it.");
  });

  it("marks replaced versions: installed ones get an update notice, others can't be installed", async () => {
    const replaced = "Replaced by 8.5.12, which fixes bugs and security issues.";
    const view = catalogView([
      packageView(
        "php",
        [
          versionView("php", "8.5.12"),
          versionView("php", "8.5.11", {
            supersededBy: "8.5.12",
            unavailableReason: replaced,
            installation: {
              id: "i-old",
              status: "installed",
              installPath: "C:\\Devluma\\bin\\php\\8.5.11-x64",
              isGlobalDefault: true,
              installedAt: null,
            },
          }),
          versionView("php", "8.4.26", { supersededBy: "8.4.27", unavailableReason: replaced }),
        ],
        { displayName: "PHP" },
      ),
    ]);
    const { container } = await renderApp("/runtimes/php", withCatalog(view));
    expect(
      await screen.findByText("Update available: 8.5.12 fixes bugs and security issues."),
    ).toBeInTheDocument();
    // The existing install keeps working: still listed, still uninstallable.
    expect(screen.getByRole("button", { name: "Uninstall PHP 8.5.11" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Install PHP 8.4.26" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Install PHP 8.5.12" })).toBeEnabled();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("handles unknown package ids", async () => {
    await renderApp("/runtimes/nope", withCatalog(catalogView([php])));
    expect(await screen.findByRole("heading", { name: "Package not found" })).toBeInTheDocument();
  });
});
