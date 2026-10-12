import { describe, expect, it, vi } from "vitest";
import { createMockAdapter, installMockAdapter, operation } from "../../test/utils";
import { unavailableAdapter } from "./adapter";
import { AppError, toAppError } from "./errors";
import { events, ipc } from "./index";

describe("toAppError", () => {
  it("keeps a well-formed backend error", () => {
    const err = toAppError({
      code: "PORT_IN_USE",
      message: "Port 80 is in use.",
      operationId: "op1",
      recoverable: true,
    });
    expect(err).toBeInstanceOf(AppError);
    expect(err).toMatchObject({
      code: "PORT_IN_USE",
      message: "Port 80 is in use.",
      operationId: "op1",
      recoverable: true,
    });
  });

  it("replaces unknown shapes with a safe INTERNAL error and does not leak the raw value", () => {
    const err = toAppError(new Error("stack trace with C:\\secret\\path"));
    expect(err.code).toBe("INTERNAL");
    expect(err.message).not.toContain("secret");
  });

  it("rejects unknown error codes", () => {
    expect(toAppError({ code: "MADE_UP", message: "x", recoverable: true }).code).toBe("INTERNAL");
  });
});

describe("ipc commands", () => {
  it("returns validated data", async () => {
    installMockAdapter(
      createMockAdapter({
        get_app_info: () => ({
          name: "devluma",
          version: "1.2.3",
          os: "windows",
          arch: "x86_64",
          debugBuild: false,
        }),
      }),
    );
    await expect(ipc.getAppInfo()).resolves.toMatchObject({ version: "1.2.3" });
  });

  it("rejects malformed responses as INTERNAL", async () => {
    installMockAdapter(createMockAdapter({ get_app_info: () => ({ version: 42 }) }));
    await expect(ipc.getAppInfo()).rejects.toMatchObject({ code: "INTERNAL" });
  });

  it("propagates typed backend errors", async () => {
    installMockAdapter(
      createMockAdapter({
        get_app_info: () => {
          throw { code: "PERMISSION_DENIED", message: "Denied.", recoverable: true };
        },
      }),
    );
    await expect(ipc.getAppInfo()).rejects.toMatchObject({ code: "PERMISSION_DENIED", message: "Denied." });
  });

  it("fails honestly outside the desktop runtime", async () => {
    installMockAdapter({ adapter: unavailableAdapter });
    await expect(ipc.getAppInfo()).rejects.toMatchObject({ code: "DESKTOP_UNAVAILABLE" });
  });
});

describe("ipc events", () => {
  it("delivers valid operation progress and drops malformed payloads", async () => {
    const mock = createMockAdapter();
    installMockAdapter(mock);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const handler = vi.fn();
    await events.onOperationProgress(handler);

    mock.emit("operation://progress", operation({ operationId: "op1", current: 1, total: 4 }));
    mock.emit("operation://progress", { operationId: "op2", status: "exploded" });

    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler).toHaveBeenCalledWith(expect.objectContaining({ operationId: "op1" }));
    expect(warn).toHaveBeenCalledOnce();
    warn.mockRestore();
  });
});
