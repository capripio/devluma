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

const firstRun = (overrides: Record<string, unknown> = {}) => ({
  completed: false,
  storage: {
    root: String.raw`D:\Tools\Devluma`,
    mode: "portable",
    exeDir: String.raw`D:\Tools\Devluma`,
    exeDirWritable: true,
  },
  canChangeStorage: true,
  freeBytes: 50_000_000_000,
  missingPrerequisite: null,
  catalogVersion: "2026.10.3",
  catalogProblems: 0,
  installedPackages: 0,
  ...overrides,
});

const catalog = catalogView([
  packageView("php", [versionView("php", "8.5.11"), versionView("php", "8.4.26")], { displayName: "PHP" }),
  packageView("nginx", [versionView("nginx", "1.30.5", { sizeBytes: 2_776_622 })], {
    displayName: "Nginx",
    category: "web_server",
  }),
  packageView("apache", [versionView("apache", "2.4.69")], {
    displayName: "Apache httpd",
    category: "web_server",
  }),
  packageView("mariadb", [versionView("mariadb", "11.8.9", { sizeBytes: 97_016_251 })], {
    displayName: "MariaDB",
    category: "database",
  }),
  packageView("mailpit", [versionView("mailpit", "1.29.2")], { displayName: "Mailpit", category: "tool" }),
]);

describe("first-run setup", () => {
  it("can be skipped, and then the app opens", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_setup_status: () => firstRun(),
      complete_setup: () => null,
    });
    await renderApp("/", mock);
    expect(screen.getByRole("heading", { level: 1, name: "Set up Devluma" })).toBeInTheDocument();
    expect(screen.getByText(/downloaded only when you choose them/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Skip setup" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "complete_setup")).toBe(true));
    expect(await screen.findByRole("navigation")).toBeInTheDocument();
  });

  it("installs a preset with sizes, a review, and progress, then opens the app", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_setup_status: () => firstRun(),
      list_packages: () => catalog,
      choose_storage: () => ({ restartNeeded: false }),
      run_setup: () => ({ operationId: "setup-1" }),
      complete_setup: () => null,
    });
    const { container } = await renderApp("/", mock);
    expect(await axeViolations(container)).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "Next" }));

    // Folder: portable by default, next to devluma.exe.
    const storage = screen.getByRole("region", { name: "Where should Devluma keep everything?" });
    expect(within(storage).getByLabelText(/Devluma's own folder/)).toBeChecked();
    expect(storage).toHaveTextContent(String.raw`D:\Tools\Devluma`);
    await userEvent.click(within(storage).getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "choose_storage")?.args).toEqual({ folder: null }),
    );

    // Tools: the PHP preset picks PHP (newest), Nginx, MariaDB and Mailpit.
    const stack = await screen.findByRole("region", { name: "What do you want to download?" });
    expect(await axeViolations(container)).toEqual([]);
    await userEvent.click(within(stack).getByLabelText("PHP web stack"));
    expect(within(stack).getByLabelText("PHP version")).toHaveValue("8.5.11");
    expect(within(stack).getByLabelText("Nginx")).toBeChecked();
    expect(within(stack).getByLabelText("MariaDB")).toBeChecked();
    expect(within(stack).getByLabelText("Mailpit")).toBeChecked();
    // Web servers are pick-one.
    await userEvent.click(within(stack).getByLabelText("Apache httpd"));
    expect(within(stack).getByLabelText("Nginx")).not.toBeChecked();
    await userEvent.click(within(stack).getByLabelText("Nginx"));
    expect(within(stack).getByLabelText("Apache httpd")).not.toBeChecked();
    expect(stack).toHaveTextContent("Download:");
    await userEvent.click(within(stack).getByRole("button", { name: "Review" }));

    const review = screen.getByRole("region", { name: "Review" });
    expect(review).toHaveTextContent("downloads.example.org");
    expect(review).toHaveTextContent("SHA-256");
    await userEvent.click(within(review).getByRole("button", { name: "Download and set up" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "run_setup")?.args).toEqual({
        plan: {
          items: [
            { packageId: "php", version: "8.5.11" },
            { packageId: "mariadb", version: "11.8.9" },
            { packageId: "mailpit", version: "1.29.2" },
            { packageId: "nginx", version: "1.30.5" },
          ],
        },
      }),
    );

    act(() =>
      mock.emit(
        "operation://progress",
        operation({
          operationId: "setup-1",
          operationType: "first_run_setup",
          phase: "Installing PHP 8.5.11",
          current: 0,
          total: 3,
        }),
      ),
    );
    expect(await screen.findByText("Installing PHP 8.5.11")).toBeInTheDocument();
    act(() =>
      mock.emit(
        "operation://progress",
        operation({
          operationId: "setup-1",
          operationType: "first_run_setup",
          status: "succeeded",
          current: 3,
          total: 3,
        }),
      ),
    );
    expect(await screen.findByRole("heading", { name: "All set" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Open Devluma" }));
    expect(await screen.findByRole("navigation")).toBeInTheDocument();
  });

  it("choosing another folder explains the restart and moves nothing", async () => {
    await renderApp(
      "/",
      createMockAdapter({
        ...defaultHandlers,
        get_setup_status: () => firstRun(),
        choose_storage: () => ({ restartNeeded: true }),
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    const storage = screen.getByRole("region", { name: "Where should Devluma keep everything?" });
    await userEvent.type(within(storage).getByLabelText("Folder path"), String.raw`E:\DevData`);
    expect(within(storage).getByLabelText("Another folder")).toBeChecked();
    await userEvent.click(within(storage).getByRole("button", { name: "Next" }));
    expect(await within(storage).findByText(/needs to restart/)).toHaveTextContent(
      "Nothing already in the current folder is moved",
    );
    expect(within(storage).getByRole("button", { name: "Restart Devluma" })).toBeInTheDocument();
  });

  it("warns about a missing prerequisite on the welcome step", async () => {
    await renderApp(
      "/",
      createMockAdapter({
        ...defaultHandlers,
        get_setup_status: () =>
          firstRun({ missingPrerequisite: "Microsoft Visual C++ 2015–2022 Redistributable (x64)" }),
      }),
    );
    expect(screen.getByText(/Redistributable \(x64\) isn't installed/)).toBeInTheDocument();
  });
});
