import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const settings = (overrides: Record<string, unknown> = {}) => ({
  appUrl: null,
  catalogUrl: null,
  currentVersion: "0.1.0",
  catalogVersion: "2026.10.3",
  pendingCatalogVersion: null,
  ...overrides,
});

describe("update settings", () => {
  it("is off until an address is set", async () => {
    await renderApp("/settings/updates");
    const section = await screen.findByRole("region", { name: "Updates" });
    expect(section).toHaveTextContent("Updates are off until an update address is set");
    expect(within(section).getByRole("button", { name: "Check for app update" })).toBeDisabled();
    expect(within(section).getByRole("button", { name: "Check for catalog update" })).toBeDisabled();
  });

  it("offers a found update behind a confirmation that explains what stops", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_update_settings: () => settings({ appUrl: "https://updates.example.com/{{target}}" }),
      check_app_update: () => ({
        currentVersion: "0.1.0",
        availableVersion: "0.2.0",
        notes: "Fixes.",
        date: null,
      }),
      install_app_update: () => null,
    });
    await renderApp("/settings/updates", mock);
    const section = await screen.findByRole("region", { name: "Updates" });
    await userEvent.click(within(section).getByRole("button", { name: "Check for app update" }));
    expect(await within(section).findByText(/is available/)).toHaveTextContent("0.2.0");
    await userEvent.click(within(section).getByRole("button", { name: "Install and restart…" }));
    const dialog = screen.getByRole("dialog", { name: "Install Devluma 0.2.0?" });
    expect(dialog).toHaveTextContent("signature checked");
    expect(dialog).toHaveTextContent("Running services and shared sites are stopped");
    await userEvent.click(within(dialog).getByRole("button", { name: "Install and restart" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "install_app_update")).toBe(true));
  });

  it("reports a verified catalog that is used after a restart", async () => {
    let pending: string | null = null;
    await renderApp(
      "/settings/updates",
      createMockAdapter({
        ...defaultHandlers,
        get_update_settings: () =>
          settings({ catalogUrl: "https://example.com/catalog.json", pendingCatalogVersion: pending }),
        check_catalog_update: () => {
          pending = "2026.11.1";
          return {
            currentVersion: "2026.10.3",
            availableVersion: "2026.11.1",
            message: "Catalog 2026.11.1 was verified and saved. Restart Devluma to use it.",
          };
        },
      }),
    );
    const section = await screen.findByRole("region", { name: "Updates" });
    await userEvent.click(within(section).getByRole("button", { name: "Check for catalog update" }));
    expect(await within(section).findByText(/Restart Devluma to use it/)).toBeInTheDocument();
    expect(await within(section).findByText(/2026.11.1 verified — used after a restart/)).toBeInTheDocument();
  });
});
