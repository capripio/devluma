import { render, screen, waitFor } from "@testing-library/react";
import axe from "axe-core";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, expect } from "vitest";
import { AppProviders } from "../app/App";
import { routes } from "../app/routes";
import { setIpcAdapter, type IpcAdapter } from "../lib/ipc/adapter";

type Listener = (payload: unknown) => void;

/** In-memory IPC adapter: per-command handlers plus the ability to emit backend events. */
export function createMockAdapter(
  handlers: Record<string, (args?: Record<string, unknown>) => unknown> = {},
) {
  const listeners = new Map<string, Set<Listener>>();
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const adapter: IpcAdapter = {
    async invoke(command, args) {
      calls.push({ command, args });
      const handler = handlers[command];
      if (!handler) throw { code: "NOT_IMPLEMENTED", message: `No mock for ${command}`, recoverable: false };
      return handler(args);
    },
    async listen(event, handler) {
      const set = listeners.get(event) ?? new Set();
      set.add(handler);
      listeners.set(event, set);
      return () => set.delete(handler);
    },
  };
  return {
    adapter,
    calls,
    emit(event: string, payload: unknown) {
      listeners.get(event)?.forEach((l) => l(payload));
    },
  };
}

let restoreAdapter: (() => void) | undefined;
afterEach(() => {
  restoreAdapter?.();
  restoreAdapter = undefined;
});

export function installMockAdapter(mock: { adapter: IpcAdapter }) {
  restoreAdapter = setIpcAdapter(mock.adapter);
}

export const readyStartup = {
  state: "ready",
  dataDir: String.raw`C:\Users\dev\AppData\Local\Devluma`,
  databasePath: String.raw`C:\Users\dev\AppData\Local\Devluma\app.db`,
  backupsDir: String.raw`C:\Users\dev\AppData\Local\Devluma\backups`,
  projectsDir: String.raw`C:\Users\dev\AppData\Local\Devluma\www`,
  recovery: { operationsNeedingRecovery: 0, installationsMarkedBroken: 0, servicesMarkedUnknown: 0 },
};

export const defaultHandlers: Record<string, (args?: Record<string, unknown>) => unknown> = {
  get_app_info: () => ({
    name: "devluma",
    version: "0.1.0",
    os: "windows",
    arch: "x86_64",
    debugBuild: true,
  }),
  get_startup_status: () => readyStartup,
  list_operations: () => [],
  list_packages: () => catalogView([]),
  list_projects: () => [],
  list_services: () => [],
  list_scheduled_jobs: () => [],
  list_sites: () => [],
  list_certificates: () => [],
  list_tunnels: () => [],
  list_database_services: () => [],
  list_database_backups: () => [],
  get_setup_status: () => ({
    completed: true,
    storage: {
      root: String.raw`C:\Devluma`,
      mode: "portable",
      exeDir: String.raw`C:\Devluma`,
      exeDirWritable: true,
    },
    canChangeStorage: true,
    freeBytes: 100_000_000_000,
    missingPrerequisite: null,
    catalogVersion: "2026.10.3",
    catalogProblems: 0,
    installedPackages: 0,
  }),
  get_update_settings: () => ({
    appUrl: null,
    catalogUrl: null,
    currentVersion: "0.1.0",
    catalogVersion: "2026.10.3",
    pendingCatalogVersion: null,
  }),
  get_preferences: () => preferencesView(),
  get_storage_usage: () => ({ root: String.raw`C:\Devluma`, folders: [] }),
  get_ngrok_status: () => ({ path: null, version: null, problem: null, tokenSaved: false, detected: [] }),
  get_hosts: () => ({
    path: String.raw`C:\Windows\System32\drivers\etc\hosts`,
    needsElevation: true,
    entries: [],
    otherCount: 0,
    strayTags: [],
    backups: [],
  }),
};

/** A `CatalogView` with the given packages. */
export function catalogView(packages: unknown[], issues: unknown[] = []) {
  return {
    catalogVersion: "test-1",
    packagesDir: String.raw`C:\Users\dev\AppData\Local\Devluma\packages`,
    packages,
    issues,
  };
}

/** A `VersionView` for package `pkg` with overridable fields. */
export function versionView(pkg: string, version: string, overrides: Record<string, unknown> = {}) {
  return {
    key: `${pkg}@${version}@x64`,
    version,
    architecture: "x64",
    sizeBytes: 31_457_280,
    sourceUrl: `https://downloads.example.org/${pkg}-${version}.zip`,
    sourceHost: "downloads.example.org",
    integrityPolicy: "sha256-required",
    sha256: "a".repeat(64),
    dependencies: [],
    conflicts: [],
    releaseDate: "2026-09-01",
    endOfLifeDate: null,
    installPath: [
      "C:",
      "Users",
      "dev",
      "AppData",
      "Local",
      "Devluma",
      "packages",
      pkg,
      `${version}-x64`,
    ].join("\\"),
    ...overrides,
  };
}

/** A `PackageView` with overridable fields. */
export function packageView(id: string, versions: unknown[], overrides: Record<string, unknown> = {}) {
  return {
    id,
    displayName: id.toUpperCase(),
    category: "runtime",
    description: `${id} for tests`,
    publisher: "Fixture Inc.",
    homepageUrl: null,
    license: { id: "MIT", url: null },
    supportPolicyUrl: null,
    versions,
    ...overrides,
  };
}

/** A valid `OperationProgress` payload with overridable fields. */
export function operation(overrides: Record<string, unknown> = {}) {
  return {
    operationId: "op-1",
    operationType: "install_package",
    status: "running",
    startedAt: "2026-10-11T10:00:00.000Z",
    ...overrides,
  };
}

/**
 * Renders the full app (startup gate + shell + routes) at `path` with the given IPC mock,
 * and waits until startup has finished.
 */
export async function renderApp(path = "/", mock = createMockAdapter(defaultHandlers)) {
  installMockAdapter(mock);
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  const result = render(
    <AppProviders>
      <RouterProvider router={router} />
    </AppProviders>,
  );
  await waitFor(() => expect(screen.queryByText("Starting Devluma…")).not.toBeInTheDocument());
  return { ...result, router, mock };
}

/** Accessibility smoke check. Color contrast is skipped: jsdom doesn't compute styles. */
export async function axeViolations(container: Element) {
  const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
  return results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length})`);
}

/** A `PreferencesView` with the defaults, plus overridable preferences and view fields. */
export function preferencesView(
  preferences: Record<string, unknown> = {},
  overrides: Record<string, unknown> = {},
) {
  return {
    preferences: {
      domainSuffix: "test",
      httpsPort: null,
      terminal: "power_shell",
      defaultPorts: {
        nginx: 8080,
        apache: 8081,
        mariadb: 3306,
        postgresql: 5432,
        mongodb: 27017,
        mailpit: 1025,
        mailpitWeb: 8025,
      },
      startServices: [],
      downloadTimeouts: { connectSecs: 15, idleSecs: 30 },
      ...preferences,
    },
    suggestedSuffixes: ["test", "localhost", "internal", "local"],
    suffixAdvice: { httpsSupported: true, warnings: [] as string[] },
    cmderInstalled: false,
    launchAtLogin: "off",
    launchAtLoginSupported: true,
    ...overrides,
  };
}
