import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  axeViolations,
  chooseFromMenu,
  createMockAdapter,
  defaultHandlers,
  renderApp,
} from "../../test/utils";

const status = (overrides: Record<string, unknown> = {}) => ({
  path: null,
  version: null,
  problem: null,
  tokenSaved: false,
  detected: [String.raw`D:\laragon\bin\ngrok\ngrok.exe`],
  ...overrides,
});

const site = {
  id: "site-1",
  projectId: "p1",
  domain: "my-shop.test",
  documentRoot: String.raw`D:\code\shop\public`,
  webServerServiceId: "w1",
  phpInstallationId: null,
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
  phpVersion: null,
  phpOutdated: false,
  configPresent: true,
};

describe("ngrok settings", () => {
  it("offers detected ngrok.exe files and keeps the token write-only", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_ngrok_status: () => status(),
      set_ngrok_path: () => status({ path: String.raw`D:\laragon\bin\ngrok\ngrok.exe`, version: "3.35.0" }),
      set_ngrok_token: () => status({ tokenSaved: true }),
    });
    const { container } = await renderApp("/settings/sharing", mock);
    const section = await screen.findByRole("region", { name: "Sharing with ngrok" });
    expect(section).toHaveTextContent("Devluma doesn't download ngrok");
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(
      within(section).getByRole("button", { name: String.raw`Use D:\laragon\bin\ngrok\ngrok.exe` }),
    );
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "set_ngrok_path")?.args).toEqual({
        path: String.raw`D:\laragon\bin\ngrok\ngrok.exe`,
      }),
    );

    const tokenForm = within(section).getByRole("form", { name: "ngrok authtoken" });
    const tokenInput = within(tokenForm).getByLabelText("Authtoken");
    expect(tokenInput).toHaveAttribute("type", "password");
    await userEvent.type(tokenInput, "2abcDEFghiJKLmnoPQRstu_vwx");
    await userEvent.click(within(tokenForm).getByRole("button", { name: "Save to Credential Manager" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "set_ngrok_token")).toBe(true));
    await waitFor(() => expect(tokenInput).toHaveValue(""));
  });
});

describe("sharing a site", () => {
  it("explains that sharing makes the site public, then shows the public address", async () => {
    let shared = false;
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_sites: () => [site],
      list_tunnels: () =>
        shared
          ? [
              {
                siteId: "site-1",
                domain: "my-shop.test",
                publicUrl: "https://abc.ngrok-free.app",
                running: true,
                startedAt: "2026-10-12T10:00:00Z",
                lastError: null,
              },
            ]
          : [],
      start_tunnel: () => {
        shared = true;
        return {
          siteId: "site-1",
          domain: "my-shop.test",
          publicUrl: "https://abc.ngrok-free.app",
          running: true,
          startedAt: "2026-10-12T10:00:00Z",
          lastError: null,
        };
      },
      stop_tunnel: () => {
        shared = false;
        return null;
      },
    });
    await renderApp("/sites", mock);
    await chooseFromMenu("More actions for my-shop.test", "Share with ngrok");
    const dialog = screen.getByRole("dialog", { name: "Share my-shop.test on the internet?" });
    expect(dialog).toHaveTextContent("Anyone who has the address can open it");
    expect(dialog).toHaveTextContent("close Devluma");
    await userEvent.click(within(dialog).getByRole("button", { name: "Share" }));
    const table = screen.getByRole("table");
    expect(await within(table).findByText("https://abc.ngrok-free.app")).toBeInTheDocument();
    expect(table).toHaveTextContent("Public");

    await userEvent.click(within(table).getByRole("button", { name: "Stop sharing my-shop.test" }));
    await waitFor(() =>
      expect(within(table).queryByText("https://abc.ngrok-free.app")).not.toBeInTheDocument(),
    );
  });
});
