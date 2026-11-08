import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const exe = String.raw`C:\Tools\frankenphp\frankenphp.exe`;

const status = (overrides: Record<string, unknown> = {}) => ({
  path: null,
  version: null,
  phpVersion: null,
  installationId: null,
  problem: null,
  services: [],
  detected: [exe],
  ...overrides,
});

const chosen = status({ path: exe, version: "1.13.5", phpVersion: "8.5.12", installationId: "i-fp" });

describe("FrankenPHP (user-provided)", () => {
  it("explains why it isn't downloaded and uses the frankenphp.exe the user picks", async () => {
    let current = status();
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_frankenphp_status: () => current,
      set_frankenphp_path: (args) => {
        expect(args).toEqual({ path: exe });
        current = chosen;
        return current;
      },
    });
    const { container } = await renderApp("/settings/frankenphp", mock);
    const section = await screen.findByRole("region", { name: "FrankenPHP" });
    expect(section).toHaveTextContent("rebuilds its Windows release every day");
    expect(section).toHaveTextContent("Not chosen");
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(section).getByRole("button", { name: `Use ${exe}` }));
    await waitFor(() => expect(section).toHaveTextContent("FrankenPHP 1.13.5 with PHP 8.5.12"));
    expect(section).toHaveTextContent("Create a FrankenPHP service");
  });

  it("shows why a chosen copy can't be used, and can't be forgotten while services use it", async () => {
    await renderApp(
      "/settings/frankenphp",
      createMockAdapter({
        ...defaultHandlers,
        get_frankenphp_status: () => ({
          ...chosen,
          services: ["fp"],
          problem: "frankenphp.exe no longer exists.",
        }),
      }),
    );
    const section = await screen.findByRole("region", { name: "FrankenPHP" });
    expect(section).toHaveTextContent("frankenphp.exe no longer exists.");
    expect(within(section).getByRole("button", { name: "Forget FrankenPHP" })).toBeDisabled();
    expect(section).toHaveTextContent("Remove its services first");
  });

  it("is offered in Add service once chosen, with its admin API port", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_frankenphp_status: () => chosen,
      create_service: () => ({ operationId: "op-fp" }),
    });
    await renderApp("/services", mock);
    await userEvent.click((await screen.findAllByRole("button", { name: "Add service" }))[0]!);
    const form = await screen.findByRole("form", { name: "Add service" });
    expect(within(form).getByLabelText("Installed version")).toHaveTextContent(
      "FrankenPHP 1.13.5 (your own copy)",
    );
    expect(form).toHaveTextContent("projects' PHP pins don't apply");
    expect(within(form).getByLabelText("Admin API port")).toHaveAttribute("placeholder", "2019");
    await userEvent.click(within(form).getByRole("button", { name: "Create service" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "create_service")?.args).toEqual({
        request: {
          installationId: "i-fp",
          name: "frankenphp",
          port: 80,
          adminPort: 2019,
          useExistingData: false,
        },
      }),
    );
  });

  it("isn't offered while the chosen copy has a problem", async () => {
    await renderApp(
      "/services",
      createMockAdapter({
        ...defaultHandlers,
        get_frankenphp_status: () => ({
          ...chosen,
          problem: "It doesn't report a FrankenPHP and PHP version.",
        }),
      }),
    );
    await userEvent.click((await screen.findAllByRole("button", { name: "Add service" }))[0]!);
    expect(
      await screen.findByText(/No web server, database or Mailpit is installed yet/),
    ).toBeInTheDocument();
  });
});
