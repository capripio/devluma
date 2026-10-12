import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";

const base = {
  certificatePath: String.raw`C:\Devluma\certificates\x\cert.pem`,
  privateKeyPath: String.raw`C:\Devluma\certificates\x\key.pem`,
  fingerprintSha256: "ab".repeat(32),
  notBefore: "2026-10-11T00:00:00Z",
  trustStore: null,
  trusted: false,
  createdAt: "2026-10-12T00:00:00Z",
  updatedAt: "2026-10-12T00:00:00Z",
};

const ca = (overrides: Record<string, unknown> = {}) => ({
  ...base,
  id: "ca",
  commonName: "Devluma Local Development CA (PC)",
  sans: [],
  issuerType: "local_ca",
  privateKeyPath: String.raw`C:\Devluma\certificates\ca\ca.key.dpapi`,
  notAfter: "2036-10-12T00:00:00Z",
  daysLeft: 3650,
  expiringSoon: false,
  expired: false,
  trustedNow: false,
  boundSites: [],
  keyProtection: "Encrypted with Windows DPAPI for your account",
  ...overrides,
});

const siteCert = (overrides: Record<string, unknown> = {}) => ({
  ...base,
  id: "c1",
  commonName: "shop.test",
  sans: ["shop.test"],
  issuerType: "local_ca_signed",
  notAfter: "2027-11-13T00:00:00Z",
  daysLeft: 397,
  expiringSoon: false,
  expired: false,
  trustedNow: false,
  boundSites: [],
  keyProtection: "Readable only by your Windows account",
  ...overrides,
});

describe("SSL certificates", () => {
  it("explains the name-constrained CA before creating it", async () => {
    const mock = createMockAdapter({ ...defaultHandlers, create_local_ca: () => ({ ...ca() }) });
    const { container } = await renderApp("/ssl", mock);
    expect(await screen.findByText(/can only sign local domains/)).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "Create local CA" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "create_local_ca")).toBe(true));
  });

  it("keeps generate, trust, remove trust and delete as separate, explained actions", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_certificates: () => [ca(), siteCert({ boundSites: ["shop.test"] })],
      trust_certificate: () => ({ ...ca(), trusted: true }),
    });
    const { container } = await renderApp("/ssl", mock);
    const caSection = await screen.findByRole("region", { name: "Local certificate authority" });
    expect(caSection).toHaveTextContent("No — browsers will warn");
    expect(caSection).toHaveTextContent("Encrypted with Windows DPAPI");
    // Can't delete a CA that still has site certificates.
    expect(within(caSection).getByRole("button", { name: "Delete CA" })).toBeDisabled();
    // A certificate in use can't be deleted either.
    expect(screen.getByRole("button", { name: "Delete shop.test" })).toBeDisabled();
    expect(await axeViolations(container)).toEqual([]);

    await userEvent.click(within(caSection).getByRole("button", { name: "Trust on this computer" }));
    const dialog = screen.getByRole("dialog", { name: "Trust Devluma's local CA on this computer?" });
    expect(dialog).toHaveTextContent("CurrentUser\\Root");
    expect(dialog).toHaveTextContent("Windows will show its own security warning");
    expect(dialog).toHaveTextContent("Firefox");
    await userEvent.click(within(dialog).getByRole("button", { name: "Trust" }));
    await waitFor(() => expect(mock.calls.some((c) => c.command === "trust_certificate")).toBe(true));
  });

  it("removing trust says keys are kept", async () => {
    await renderApp(
      "/ssl",
      createMockAdapter({
        ...defaultHandlers,
        list_certificates: () => [ca({ trustedNow: true, trusted: true })],
      }),
    );
    await userEvent.click(await screen.findByRole("button", { name: "Remove trust" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("Keys and certificate files are kept");
  });

  it("flags expiring certificates and reports refused domains", async () => {
    await renderApp(
      "/ssl",
      createMockAdapter({
        ...defaultHandlers,
        list_certificates: () => [ca(), siteCert({ daysLeft: 12, expiringSoon: true })],
        create_dev_certificate: () => {
          throw {
            code: "VALIDATION_ERROR",
            message: "Devluma's local CA only issues certificates for local domains. bank.com isn't one.",
            recoverable: false,
          };
        },
      }),
    );
    expect(await screen.findByText("Expires soon — renew")).toBeInTheDocument();
    const form = screen.getByRole("form", { name: "Generate certificate" });
    await userEvent.type(within(form).getByLabelText(/Domains/), "bank.com");
    await userEvent.click(within(form).getByRole("button", { name: "Generate certificate" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("bank.com isn't one");
  });
});
