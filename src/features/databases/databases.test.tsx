import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const pg = {
  serviceId: "pg1",
  name: "pg",
  engine: "postgresql",
  version: "18.6",
  observedState: "running",
  healthStatus: "healthy",
  dataPath: String.raw`C:\Devluma\data\postgresql\pg`,
  dataInitialized: true,
  connection: {
    host: "127.0.0.1",
    port: 5432,
    user: "postgres",
    url: "postgresql://postgres@127.0.0.1:5432/postgres",
    cli: String.raw`"C:\Devluma\packages\postgresql\18.6-x64\bin\psql.exe" -h 127.0.0.1 -p 5432 -U postgres`,
    credentialNote: "postgres signs in without a password from this computer (trust authentication).",
  },
  logicalBackups: true,
};

const backup = {
  id: "b1",
  serviceInstanceId: "pg1",
  serviceName: "pg",
  engine: "postgresql",
  kind: "logical",
  databaseName: "shop",
  path: String.raw`C:\Devluma\backups\databases\pg1\shop-20261012.dump`,
  sizeBytes: 2048,
  sha256: "ab",
  note: null,
  operationId: "op-1",
  createdAt: "2026-10-12T10:00:00Z",
};

const plan = (overwrites: boolean) => ({
  backup,
  serviceId: "pg1",
  serviceName: "pg",
  targetDatabase: overwrites ? "shop" : "shop_copy",
  overwrites,
  effects: overwrites
    ? [
        "The database shop on pg is replaced by the backup's contents.",
        "First, shop is backed up automatically; if the restore fails, that backup is put back.",
      ]
    : ["A new database shop_copy is created on pg from the backup."],
  blocked: null,
});

describe("databases", () => {
  it("shows connection details honestly and backs up a database", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_database_services: () => [pg],
      list_databases: () => ["postgres", "shop"],
      backup_database: () => backup,
    });
    const { container } = await renderApp("/databases", mock);
    const card = await screen.findByRole("region", { name: /pg/ });
    expect(card).toHaveTextContent("postgresql://postgres@127.0.0.1:5432/postgres");
    expect(card).toHaveTextContent("without a password");
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.selectOptions(await within(card).findByLabelText("Database"), "shop");
    await userEvent.click(within(card).getByRole("button", { name: "Back up shop on pg" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "backup_database")?.args).toEqual({
        serviceId: "pg1",
        database: "shop",
      }),
    );
    expect(await within(card).findByText(/Saved to/)).toBeInTheDocument();
  });

  it("requires an explicit confirmation before a restore replaces data", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_database_services: () => [pg],
      list_databases: () => ["shop"],
      list_database_backups: () => [backup],
      preview_restore: (args) =>
        plan(((args?.request as { targetDatabase?: string }).targetDatabase ?? "shop") === "shop"),
      restore_database: () => plan(true),
    });
    await renderApp("/databases", mock);
    await userEvent.click(await screen.findByRole("button", { name: /Restore backup from/ }));
    const dialog = await screen.findByRole("dialog", { name: "Restore shop?" });
    expect(await within(dialog).findByText(/replaced by the backup's contents/)).toBeInTheDocument();
    expect(dialog).toHaveTextContent("backed up automatically");
    const restore = within(dialog).getByRole("button", { name: "Restore" });
    expect(restore).toBeDisabled();
    await userEvent.click(within(dialog).getByLabelText(/I understand this replaces/));
    expect(restore).toBeEnabled();
    await userEvent.click(restore);
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "restore_database")?.args).toEqual({
        request: { backupId: "b1", serviceId: "pg1", targetDatabase: "shop", confirmOverwrite: true },
      }),
    );
  });

  it("restoring into a new database needs no overwrite confirmation", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_database_services: () => [pg],
      list_databases: () => ["shop"],
      list_database_backups: () => [backup],
      preview_restore: (args) =>
        plan(((args?.request as { targetDatabase?: string }).targetDatabase ?? "shop") === "shop"),
    });
    await renderApp("/databases", mock);
    await userEvent.click(await screen.findByRole("button", { name: /Restore backup from/ }));
    const dialog = await screen.findByRole("dialog");
    const target = within(dialog).getByLabelText("Into database");
    await userEvent.clear(target);
    await userEvent.type(target, "shop_copy");
    expect(await within(dialog).findByText(/A new database shop_copy is created/)).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(/I understand this replaces/)).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Restore" })).toBeEnabled();
  });
});
