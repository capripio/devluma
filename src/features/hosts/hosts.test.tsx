import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const view = {
  path: String.raw`C:\Windows\System32\drivers\etc\hosts`,
  needsElevation: true,
  entries: [
    { line: 3, ip: "10.0.0.5", hostnames: ["nas.lan"], comment: "my NAS", managed: false },
    { line: 5, ip: "127.0.0.1", hostnames: ["shop.test"], comment: null, managed: true },
  ],
  raw: "10.0.0.5 nas.lan # my NAS\r\n",
  backups: [
    {
      id: "b1",
      originalPath: String.raw`C:\Windows\System32\drivers\etc\hosts`,
      backupPath: String.raw`C:\Devluma\backups\hosts\hosts-1.bak`,
      sha256Before: "x",
      sha256After: "y",
      reason: "Site shop.test",
      operationId: "op",
      createdAt: "2026-10-12T10:00:00.000Z",
      changes: [{ domain: "shop.test", ipAddress: "127.0.0.1", action: "add" }],
    },
  ],
  problem: null,
};

const plan = {
  path: view.path,
  sha256Before: "sha-now",
  entries: [],
  changes: [{ domain: "shop.test", ipAddress: "127.0.0.1", action: "remove" }],
  diff: "--- hosts\n+++ hosts\n-127.0.0.1 shop.test\n",
  needsElevation: true,
  unchanged: false,
};

describe("hosts file", () => {
  it("distinguishes Devluma's entries from everyone else's", async () => {
    const { container } = await renderApp(
      "/hosts",
      createMockAdapter({ ...defaultHandlers, get_hosts: () => view }),
    );
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("Other / you");
    expect(within(rows[0]!).queryByRole("button")).not.toBeInTheDocument();
    expect(rows[1]).toHaveTextContent("Devluma");
    expect(within(rows[1]!).getByRole("button", { name: "Remove shop.test" })).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(screen.getByRole("button", { name: "Show raw file" }));
    expect(screen.getByLabelText("Raw hosts file")).toHaveTextContent("nas.lan");
  });

  it("removes an entry only after showing the exact diff, pinning the previewed file", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_hosts: () => view,
      preview_hosts_change: () => plan,
      apply_hosts_change: () => ({ operationId: "op-3" }),
    });
    await renderApp("/hosts", mock);
    await userEvent.click(await screen.findByRole("button", { name: "Remove shop.test" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByLabelText("Changes")).toHaveTextContent("-127.0.0.1 shop.test");
    expect(dialog).toHaveTextContent("Windows will ask for administrator approval");
    expect(dialog).toHaveTextContent("backed up before writing");
    await userEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "apply_hosts_change")).toBe(true));
    expect(mock.calls.find((c) => c.command === "apply_hosts_change")?.args).toEqual({
      request: { removeDomains: ["shop.test"], expectedSha256: "sha-now" },
    });
  });

  it("restores Devluma's entries from a backup through the same preview", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      get_hosts: () => view,
      preview_hosts_change: () => ({ ...plan, unchanged: true, diff: "" }),
    });
    await renderApp("/hosts", mock);
    await userEvent.click(await screen.findByRole("button", { name: /Restore entries from backup/ }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("Nothing would change.")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(mock.calls.find((c) => c.command === "preview_hosts_change")?.args).toEqual({
      request: { restoreBackupId: "b1" },
    });
  });

  it("reports a damaged Devluma section instead of guessing", async () => {
    await renderApp(
      "/hosts",
      createMockAdapter({
        ...defaultHandlers,
        get_hosts: () => ({
          ...view,
          entries: [],
          problem: "the hosts file has a Devluma section without a matching end marker",
        }),
      }),
    );
    expect(await screen.findByRole("region", { name: "Hosts file problem" })).toHaveTextContent(
      "without a matching end marker",
    );
  });
});
