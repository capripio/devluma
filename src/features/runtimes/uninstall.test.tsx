import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  catalogView,
  createMockAdapter,
  defaultHandlers,
  packageView,
  renderApp,
  versionView,
} from "../../test/utils";

const catalog = catalogView([
  packageView(
    "mariadb",
    [
      versionView("mariadb", "11.8.9", {
        installation: {
          id: "i-db",
          status: "installed",
          installPath: String.raw`C:\Devluma\packages\mariadb\11.8.9-x64`,
          isGlobalDefault: false,
          installedAt: null,
        },
      }),
    ],
    { displayName: "MariaDB", category: "database" },
  ),
]);

const plan = (overrides: Record<string, unknown> = {}) => ({
  installationId: "i-db",
  packageId: "mariadb",
  version: "11.8.9",
  installPath: String.raw`C:\Devluma\packages\mariadb\11.8.9-x64`,
  blockingServices: [],
  blockingSites: [],
  pinsCleared: [],
  clearsGlobalDefault: false,
  backendsRemoved: 0,
  kept: ["Database data folders", "Your projects and their files", "Backups, certificates and logs"],
  canUninstall: true,
  ...overrides,
});

describe("uninstall", () => {
  it("is blocked while a service uses the version", async () => {
    await renderApp(
      "/runtimes/mariadb",
      createMockAdapter({
        ...defaultHandlers,
        list_packages: () => catalog,
        preview_uninstall: () => plan({ blockingServices: ["db"], canUninstall: false }),
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Uninstall MariaDB 11.8.9" }));
    const dialog = screen.getByRole("dialog");
    expect(await within(dialog).findByText(/Service db — remove it first/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Uninstall" })).toBeDisabled();
  });

  it("says data is kept, then uninstalls", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_packages: () => catalog,
      preview_uninstall: () => plan(),
      uninstall_package: () => ({}),
    });
    await renderApp("/runtimes/mariadb", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Uninstall MariaDB 11.8.9" }));
    const dialog = screen.getByRole("dialog");
    expect(await within(dialog).findByText(/Kept: Database data folders/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "uninstall_package")?.args).toEqual({
        installationId: "i-db",
      }),
    );
  });
});
