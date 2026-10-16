import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const project = {
  id: "p1",
  name: "inventory",
  rootPath: String.raw`C:\Projects\inventory`,
  projectType: "laravel",
  localDomain: null,
  notes: null,
  isFavorite: false,
  createdAt: "2026-10-01T00:00:00Z",
  updatedAt: "2026-10-01T00:00:00Z",
  lastOpenedAt: null,
  detection: { kind: "laravel", label: "Laravel", suggestedDocumentRoot: "public", virtualEnvScripts: null },
  runtimes: [
    {
      packageId: "php",
      packageName: "PHP",
      effective: {
        installationId: "php83",
        version: "8.3.14",
        installPath: String.raw`C:\Devluma\packages\php\8.3.14`,
        source: "project_pin",
      },
    },
  ],
};

describe("stack map", () => {
  it("selects a project line and opens a terminal set up for it", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project],
      open_project_terminal: () => ({
        program: "power_shell",
        pathPrefix: [String.raw`C:\Devluma\packages\php\8.3.14`],
      }),
    });
    await renderApp("/", mock);

    await userEvent.click(await screen.findByRole("button", { name: "Project inventory" }));
    const pane = screen.getByRole("complementary", { name: "Station details" });
    expect(within(pane).getByRole("heading", { name: "inventory" })).toBeInTheDocument();
    expect(pane).toHaveTextContent("pinned for this project");

    await userEvent.click(within(pane).getByRole("button", { name: "Open terminal" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "open_project_terminal")?.args).toEqual({
        projectId: "p1",
      }),
    );
    expect(await within(pane).findByText(/first on PATH/)).toBeInTheDocument();
  });
});
