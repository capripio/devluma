import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const preview = (masked: boolean) => ({
  sections: [
    {
      file: "app.txt",
      title: "App and system",
      content: masked
        ? String.raw`Data folder: %LOCALAPPDATA%\Devluma`
        : String.raw`Data folder: C:\Users\sam\AppData\Local\Devluma`,
    },
    { file: "operations.txt", title: "Recent operations", content: "start_tunnel failed — bad token [REDACTED]" },
  ],
  neverIncluded: ["Your project files and database data", "The ngrok authtoken and other secrets"],
  pathsMasked: masked,
});

describe("logs and diagnostics", () => {
  it("exports only after a preview, which shows every section and what's never included", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      preview_diagnostics: (args) => preview(Boolean(args?.maskPaths)),
      export_diagnostics: () => String.raw`C:\Devluma\diagnostics\devluma-diagnostics-1.zip`,
    });
    const { container } = await renderApp("/logs", mock);
    const section = await screen.findByRole("region", { name: "Diagnostic export" });
    const exportButton = within(section).getByRole("button", { name: "Export diagnostics" });
    expect(exportButton).toBeDisabled();
    expect(within(section).getByLabelText(/Hide my Windows user folder/)).toBeChecked();

    await userEvent.click(within(section).getByRole("button", { name: "Preview export" }));
    expect(await within(section).findByText(/Never included:/)).toBeInTheDocument();
    expect(section).toHaveTextContent("The ngrok authtoken and other secrets");
    expect(section).toHaveTextContent("App and system");
    expect(section).toHaveTextContent("%LOCALAPPDATA%");
    expect(mock.calls.find((c) => c.command === "preview_diagnostics")?.args).toEqual({ maskPaths: true });
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(exportButton);
    expect(await within(section).findByText(/devluma-diagnostics-1.zip/)).toBeInTheDocument();
    expect(mock.calls.find((c) => c.command === "export_diagnostics")?.args).toEqual({ maskPaths: true });
  });

  it("changing the masking option drops the old preview", async () => {
    await renderApp(
      "/logs",
      createMockAdapter({ ...defaultHandlers, preview_diagnostics: (args) => preview(Boolean(args?.maskPaths)) }),
    );
    const section = await screen.findByRole("region", { name: "Diagnostic export" });
    await userEvent.click(within(section).getByRole("button", { name: "Preview export" }));
    await within(section).findByText(/Never included:/);
    await userEvent.click(within(section).getByLabelText(/Hide my Windows user folder/));
    await waitFor(() => expect(within(section).queryByText(/Never included:/)).not.toBeInTheDocument());
    expect(within(section).getByRole("button", { name: "Export diagnostics" })).toBeDisabled();
  });
});
