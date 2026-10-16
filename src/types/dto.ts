// Shared DTOs. Schemas validate payloads crossing the IPC boundary; types are inferred
// from them so the two never drift. Field names match the Rust `serde(rename_all = "camelCase")`.
import { z } from "zod";

export const appInfoSchema = z.object({
  name: z.string(),
  version: z.string(),
  os: z.string(),
  arch: z.string(),
  debugBuild: z.boolean(),
});
export type AppInfo = z.infer<typeof appInfoSchema>;

export const OPERATION_STATUSES = [
  "queued",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "needs_recovery",
] as const;
export type OperationStatus = (typeof OPERATION_STATUSES)[number];

// schema.md §7 `OperationProgress`, extended with type and timestamps. Used both as the
// `operation://progress` payload and as a `list_operations` item (`services/journal.rs`).
export const operationProgressSchema = z.object({
  operationId: z.string().min(1),
  operationType: z.string(),
  targetId: z.string().optional(),
  status: z.enum(OPERATION_STATUSES),
  phase: z.string().optional(),
  current: z.number().int().nonnegative().optional(),
  total: z.number().int().positive().optional(),
  safeMessage: z.string().optional(),
  errorCode: z.string().optional(),
  startedAt: z.string(),
  finishedAt: z.string().optional(),
});
export type OperationProgress = z.infer<typeof operationProgressSchema>;

const ipcErrorShape = z.object({
  code: z.string(),
  message: z.string(),
  recoverable: z.boolean(),
});

// `state.rs` `StartupStatus`.
export const startupStatusSchema = z.object({
  state: z.enum(["ready", "recovery_required"]),
  dataDir: z.string(),
  databasePath: z.string(),
  backupsDir: z.string(),
  error: ipcErrorShape.optional(),
  recovery: z
    .object({
      operationsNeedingRecovery: z.number().int().nonnegative(),
      installationsMarkedBroken: z.number().int().nonnegative(),
      servicesMarkedUnknown: z.number().int().nonnegative(),
    })
    .optional(),
  migrationBackupPath: z.string().optional(),
});
export type StartupStatus = z.infer<typeof startupStatusSchema>;

// trd.md §7 service supervisor states.
export const SERVICE_STATES = [
  "not_installed",
  "stopped",
  "starting",
  "running",
  "degraded",
  "stopping",
  "failed",
  "unknown",
] as const;
export type ServiceState = (typeof SERVICE_STATES)[number];

// `commands/packages.rs` `CatalogView`.
export const PACKAGE_CATEGORIES = [
  "runtime",
  "web_server",
  "database",
  "cache",
  "mail",
  "tunnel",
  "tool",
] as const;
export type PackageCategory = (typeof PACKAGE_CATEGORIES)[number];

export const ARCHITECTURES = ["x64", "arm64", "any"] as const;
export type Architecture = (typeof ARCHITECTURES)[number];

export const INSTALL_STATUSES = ["installing", "installed", "broken", "uninstalling", "removed"] as const;
export type InstallStatus = (typeof INSTALL_STATUSES)[number];

const versionViewSchema = z.object({
  key: z.string(),
  version: z.string(),
  architecture: z.enum(ARCHITECTURES),
  sizeBytes: z.number().int().positive(),
  sourceUrl: z.string(),
  sourceHost: z.string(),
  integrityPolicy: z.enum(["sha256-required", "sha256+signature"]),
  sha256: z.string(),
  dependencies: z.array(z.string()),
  conflicts: z.array(z.string()),
  releaseDate: z.string().nullable(),
  endOfLifeDate: z.string().nullable(),
  installPath: z.string(),
  unavailableReason: z.string().optional(),
  installation: z
    .object({
      id: z.string(),
      status: z.enum(INSTALL_STATUSES),
      installPath: z.string(),
      isGlobalDefault: z.boolean(),
      installedAt: z.string().nullable(),
    })
    .optional(),
});
export type VersionView = z.infer<typeof versionViewSchema>;

const packageViewSchema = z.object({
  id: z.string(),
  displayName: z.string(),
  category: z.enum(PACKAGE_CATEGORIES),
  description: z.string().nullable(),
  publisher: z.string(),
  homepageUrl: z.string().nullable(),
  license: z.object({ id: z.string(), url: z.string().nullable().optional() }),
  supportPolicyUrl: z.string().nullable(),
  versions: z.array(versionViewSchema),
});
export type PackageView = z.infer<typeof packageViewSchema>;

export const catalogViewSchema = z.object({
  catalogVersion: z.string(),
  packagesDir: z.string(),
  packages: z.array(packageViewSchema),
  issues: z.array(
    z.object({
      packageId: z.string().optional(),
      version: z.string().optional(),
      message: z.string(),
    }),
  ),
});
export type CatalogView = z.infer<typeof catalogViewSchema>;

export const installStartedSchema = z.object({ operationId: z.string() });

// `services/runtime.rs`.
export const selectionSchema = z.object({
  installationId: z.string(),
  version: z.string(),
  installPath: z.string(),
  source: z.enum(["project_pin", "global_default"]),
});
export type Selection = z.infer<typeof selectionSchema>;

export const switchPreviewSchema = z.object({
  scope: z.enum(["global", "project"]),
  packageId: z.string(),
  packageName: z.string(),
  projectName: z.string().optional(),
  current: selectionSchema.nullable(),
  target: selectionSchema.nullable(),
  changes: z.array(z.string()),
  unchanged: z.array(z.string()),
  noOp: z.boolean(),
});
export type SwitchPreview = z.infer<typeof switchPreviewSchema>;

export interface SwitchRequest {
  packageId: string;
  scope: "global" | "project";
  projectId?: string;
  /** Omit with scope "project" to stop pinning (follow the default). */
  installationId?: string;
}

// `services/php.rs` `PhpDetails`.
export const phpDetailsSchema = z.object({
  installPath: z.string(),
  iniPath: z.string(),
  iniExists: z.boolean(),
  availableTemplates: z.array(z.enum(["development", "production"])),
  extensionDir: z.string(),
  loadedModules: z.array(z.string()),
  availableExtensions: z.array(z.string()),
  startupWarnings: z.array(z.string()),
});
export type PhpDetails = z.infer<typeof phpDetailsSchema>;

// `commands/projects.rs` `ProjectView`.
export const projectSchema = z.object({
  id: z.string(),
  name: z.string(),
  rootPath: z.string(),
  projectType: z.string().nullable(),
  localDomain: z.string().nullable(),
  notes: z.string().nullable(),
  isFavorite: z.boolean(),
  createdAt: z.string(),
  updatedAt: z.string(),
  lastOpenedAt: z.string().nullable(),
});
export const projectViewSchema = projectSchema.extend({
  detection: z.object({
    kind: z.string().nullable(),
    label: z.string(),
    suggestedDocumentRoot: z.string(),
    virtualEnvScripts: z.string().nullable(),
  }),
  runtimes: z.array(
    z.object({
      packageId: z.string(),
      packageName: z.string(),
      effective: selectionSchema.nullable(),
    }),
  ),
});
export type ProjectView = z.infer<typeof projectViewSchema>;

// `commands/services.rs` / `services/supervisor`.
export const serviceViewSchema = z.object({
  id: z.string(),
  packageInstallationId: z.string(),
  instanceName: z.string(),
  serviceType: z.string(),
  lifecycleAdapter: z.string(),
  desiredState: z.string(),
  observedState: z.enum(SERVICE_STATES),
  port: z.number().int().nullable(),
  bindAddress: z.string().nullable(),
  configPath: z.string().nullable(),
  dataPath: z.string().nullable(),
  pid: z.number().int().nullable(),
  processStartedAt: z.string().nullable(),
  healthStatus: z.string(),
  lastExitCode: z.number().int().nullable(),
  lastErrorCode: z.string().nullable(),
  jobName: z.string().nullable(),
  adminPort: z.number().int().nullable(),
  updatedAt: z.string(),
  role: z.enum(["web_server", "database", "fast_cgi", "mail"]).nullable(),
  packageId: z.string(),
  packageName: z.string(),
  version: z.string(),
});
export type ServiceView = z.infer<typeof serviceViewSchema>;

export const serviceStatusSchema = z.object({
  serviceId: z.string(),
  observedState: z.enum(SERVICE_STATES),
  healthStatus: z.string(),
  pid: z.number().int().nullable(),
  lastExitCode: z.number().int().nullable(),
  lastErrorCode: z.string().nullable(),
});
export type ServiceStatus = z.infer<typeof serviceStatusSchema>;

export const serviceDetailsSchema = z.object({
  serviceDir: z.string(),
  configPath: z.string(),
  configText: z.string().nullable(),
  dataPath: z.string().nullable(),
  logs: z.array(z.object({ label: z.string(), path: z.string(), lines: z.array(z.string()) })),
});
export type ServiceDetails = z.infer<typeof serviceDetailsSchema>;

export interface CreateServiceRequest {
  installationId: string;
  name: string;
  port: number;
  adminPort?: number;
  dataPath?: string;
  useExistingData: boolean;
}

export const operationStartedSchema = z.object({ operationId: z.string() });

// `services/hosts.rs`.
export const hostsPlanSchema = z.object({
  path: z.string(),
  sha256Before: z.string(),
  entries: z.array(z.object({ ip: z.string(), domain: z.string() })),
  changes: z.array(z.object({ domain: z.string(), ipAddress: z.string(), action: z.string() })),
  diff: z.string(),
  needsElevation: z.boolean(),
  unchanged: z.boolean(),
});
export type HostsPlan = z.infer<typeof hostsPlanSchema>;

const hostsLineSchema = z.object({
  line: z.number().int(),
  ip: z.string(),
  hostnames: z.array(z.string()),
  comment: z.string().nullable(),
  managed: z.boolean(),
});
export const hostsViewSchema = z.object({
  path: z.string(),
  needsElevation: z.boolean(),
  /** Devluma's own lines only (tagged `# managed by devluma`). */
  entries: z.array(hostsLineSchema),
  /** Other entries in the file; Devluma never lists or edits them. */
  otherCount: z.number().int().nonnegative(),
  /** Devluma's tag on a non-loopback line: left alone and reported. */
  strayTags: z.array(hostsLineSchema),
  backups: z.array(
    z.object({
      id: z.string(),
      originalPath: z.string(),
      backupPath: z.string(),
      sha256Before: z.string(),
      sha256After: z.string().nullable(),
      reason: z.string(),
      operationId: z.string().nullable(),
      createdAt: z.string(),
      changes: z.array(z.object({ domain: z.string(), ipAddress: z.string(), action: z.string() })),
    }),
  ),
});
export type HostsView = z.infer<typeof hostsViewSchema>;

export interface HostsChangeRequest {
  removeDomains?: string[];
  restoreBackupId?: string;
  expectedSha256?: string;
}

// `services/sites.rs`.
export const sitePlanSchema = z.object({
  siteId: z.string(),
  domain: z.object({ normalized: z.string(), warnings: z.array(z.string()) }),
  url: z.string(),
  documentRoot: z.string(),
  configPath: z.string(),
  configText: z.string(),
  configDiff: z.string(),
  php: z.object({ installationId: z.string(), version: z.string(), port: z.number().int() }).nullable(),
  https: z
    .object({
      port: z.number().int(),
      url: z.string(),
      certificateId: z.string().nullable(),
      caExists: z.boolean(),
      caTrusted: z.boolean(),
    })
    .nullable(),
  hosts: hostsPlanSchema.nullable(),
  webServerRunning: z.boolean(),
  warnings: z.array(z.string()),
});
export type SitePlan = z.infer<typeof sitePlanSchema>;

export const siteViewSchema = z.object({
  id: z.string(),
  projectId: z.string(),
  domain: z.string(),
  documentRoot: z.string(),
  webServerServiceId: z.string(),
  phpInstallationId: z.string().nullable(),
  httpPort: z.number().int(),
  httpsPort: z.number().int().nullable(),
  sslEnabled: z.boolean(),
  configPath: z.string(),
  configHash: z.string().nullable(),
  status: z.string(),
  hostsManaged: z.boolean(),
  createdAt: z.string(),
  updatedAt: z.string(),
  projectName: z.string(),
  webServerName: z.string(),
  webServerRunning: z.boolean(),
  url: z.string(),
  httpsUrl: z.string().nullable(),
  phpVersion: z.string().nullable(),
  phpOutdated: z.boolean(),
  configPresent: z.boolean(),
});
export type SiteView = z.infer<typeof siteViewSchema>;

export const siteCheckSchema = z.object({
  url: z.string(),
  status: z.number().int().nullable(),
  message: z.string(),
});
export type SiteCheck = z.infer<typeof siteCheckSchema>;

export interface SiteRequest {
  siteId?: string;
  projectId: string;
  domain: string;
  webServerServiceId: string;
  usePhp: boolean;
  documentRoot?: string;
  manageHosts: boolean;
  expectedHostsSha256?: string;
  https: boolean;
}

// `services/setup.rs`, `services/storage.rs`.
export const setupStatusSchema = z.object({
  completed: z.boolean(),
  storage: z.object({
    root: z.string(),
    mode: z.enum(["portable", "custom", "profile"]),
    exeDir: z.string().nullable(),
    exeDirWritable: z.boolean(),
  }),
  canChangeStorage: z.boolean(),
  freeBytes: z.number().int().nullable(),
  missingPrerequisite: z.string().nullable(),
  catalogVersion: z.string(),
  catalogProblems: z.number().int(),
  installedPackages: z.number().int(),
});
export type SetupStatus = z.infer<typeof setupStatusSchema>;

// `services/updates.rs`, `commands/updates.rs`.
export const updateSettingsSchema = z.object({
  appUrl: z.string().nullable(),
  catalogUrl: z.string().nullable(),
  currentVersion: z.string(),
  catalogVersion: z.string(),
  pendingCatalogVersion: z.string().nullable(),
});
export type UpdateSettings = z.infer<typeof updateSettingsSchema>;
export const catalogCheckSchema = z.object({
  currentVersion: z.string(),
  availableVersion: z.string().nullable(),
  message: z.string(),
});
export const appUpdateSchema = z.object({
  currentVersion: z.string(),
  availableVersion: z.string().nullable(),
  notes: z.string().nullable(),
  date: z.string().nullable(),
});
export type AppUpdate = z.infer<typeof appUpdateSchema>;

// `services/diagnostics.rs`.
export const diagnosticsPreviewSchema = z.object({
  sections: z.array(z.object({ file: z.string(), title: z.string(), content: z.string() })),
  neverIncluded: z.array(z.string()),
  pathsMasked: z.boolean(),
});
export type DiagnosticsPreview = z.infer<typeof diagnosticsPreviewSchema>;

// `services/uninstall.rs`.
export const uninstallPlanSchema = z.object({
  installationId: z.string(),
  packageId: z.string(),
  version: z.string(),
  installPath: z.string(),
  blockingServices: z.array(z.string()),
  blockingSites: z.array(z.string()),
  pinsCleared: z.array(z.string()),
  clearsGlobalDefault: z.boolean(),
  backendsRemoved: z.number().int(),
  kept: z.array(z.string()),
  canUninstall: z.boolean(),
});
export type UninstallPlan = z.infer<typeof uninstallPlanSchema>;

// `services/databases.rs`.
export const databaseServiceSchema = z.object({
  serviceId: z.string(),
  name: z.string(),
  engine: z.enum(["mariadb", "postgresql", "mongodb"]),
  version: z.string(),
  observedState: z.string(),
  healthStatus: z.string(),
  dataPath: z.string().nullable(),
  dataInitialized: z.boolean(),
  connection: z
    .object({
      host: z.string(),
      port: z.number().int(),
      user: z.string().nullable(),
      url: z.string(),
      cli: z.string().nullable(),
      credentialNote: z.string(),
    })
    .nullable(),
  logicalBackups: z.boolean(),
});
export type DatabaseServiceView = z.infer<typeof databaseServiceSchema>;
export const backupRecordSchema = z.object({
  id: z.string(),
  serviceInstanceId: z.string().nullable(),
  serviceName: z.string(),
  engine: z.string(),
  kind: z.enum(["logical", "snapshot"]),
  databaseName: z.string().nullable(),
  path: z.string(),
  sizeBytes: z.number().int(),
  sha256: z.string().nullable(),
  note: z.string().nullable(),
  operationId: z.string().nullable(),
  createdAt: z.string(),
});
export type BackupRecord = z.infer<typeof backupRecordSchema>;
export const restorePlanSchema = z.object({
  backup: backupRecordSchema,
  serviceId: z.string(),
  serviceName: z.string(),
  targetDatabase: z.string().nullable(),
  overwrites: z.boolean(),
  effects: z.array(z.string()),
  blocked: z.string().nullable(),
});
export type RestorePlan = z.infer<typeof restorePlanSchema>;
export interface RestoreRequest {
  backupId: string;
  serviceId?: string;
  targetDatabase?: string;
  confirmOverwrite: boolean;
}

// `services/tunnels.rs`.
export const ngrokStatusSchema = z.object({
  path: z.string().nullable(),
  version: z.string().nullable(),
  problem: z.string().nullable(),
  tokenSaved: z.boolean(),
  detected: z.array(z.string()),
});
export type NgrokStatus = z.infer<typeof ngrokStatusSchema>;
export const tunnelViewSchema = z.object({
  siteId: z.string(),
  domain: z.string(),
  publicUrl: z.string().nullable(),
  running: z.boolean(),
  startedAt: z.string(),
  lastError: z.string().nullable(),
});
export type TunnelView = z.infer<typeof tunnelViewSchema>;

// `services/certificates.rs` `CertificateView`.
export const certificateSchema = z.object({
  id: z.string(),
  commonName: z.string(),
  sans: z.array(z.string()),
  certificatePath: z.string(),
  privateKeyPath: z.string(),
  issuerType: z.enum(["local_ca", "local_ca_signed"]),
  fingerprintSha256: z.string(),
  notBefore: z.string(),
  notAfter: z.string(),
  trustStore: z.string().nullable(),
  trusted: z.boolean(),
  createdAt: z.string(),
  updatedAt: z.string(),
});
export const certificateViewSchema = certificateSchema.extend({
  daysLeft: z.number().int(),
  expiringSoon: z.boolean(),
  expired: z.boolean(),
  trustedNow: z.boolean(),
  boundSites: z.array(z.string()),
  keyProtection: z.string(),
});
export type CertificateView = z.infer<typeof certificateViewSchema>;

// Settings (ADR 0015) — mirrors `services::preferences` and `commands::settings`.
export const terminalProgramSchema = z.enum(["power_shell", "command_prompt", "cmder"]);
export type TerminalProgram = z.infer<typeof terminalProgramSchema>;
export const defaultPortsSchema = z.object({
  nginx: z.number(),
  apache: z.number(),
  mariadb: z.number(),
  postgresql: z.number(),
  mongodb: z.number(),
  mailpit: z.number(),
  mailpitWeb: z.number(),
});
export type DefaultPorts = z.infer<typeof defaultPortsSchema>;
export const preferencesSchema = z.object({
  domainSuffix: z.string(),
  httpsPort: z.number().nullable(),
  terminal: terminalProgramSchema,
  defaultPorts: defaultPortsSchema,
  startServices: z.array(z.string()),
  downloadTimeouts: z.object({ connectSecs: z.number(), idleSecs: z.number() }),
});
export type Preferences = z.infer<typeof preferencesSchema>;
export const launchAtLoginSchema = z.enum(["off", "on", "other_copy"]);
export type LaunchAtLogin = z.infer<typeof launchAtLoginSchema>;
export const preferencesViewSchema = z.object({
  preferences: preferencesSchema,
  suggestedSuffixes: z.array(z.string()),
  suffixAdvice: z.object({ httpsSupported: z.boolean(), warnings: z.array(z.string()) }),
  cmderInstalled: z.boolean(),
  launchAtLogin: launchAtLoginSchema,
  launchAtLoginSupported: z.boolean(),
});
export type PreferencesView = z.infer<typeof preferencesViewSchema>;
export const terminalOpenedSchema = z.object({
  program: terminalProgramSchema,
  pathPrefix: z.array(z.string()),
});
export const storageUsageSchema = z.object({
  root: z.string(),
  folders: z.array(z.object({ key: z.string(), path: z.string(), bytes: z.number(), files: z.number() })),
});
export type StorageUsage = z.infer<typeof storageUsageSchema>;
