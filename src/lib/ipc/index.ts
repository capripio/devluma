// Typed IPC surface. Command and event names appear only in this file; UI code calls
// `ipc.*` / `events.*`. Every result and event payload is runtime-validated.
import { z } from "zod";
import {
  appInfoSchema,
  catalogViewSchema,
  installStartedSchema,
  operationProgressSchema,
  phpDetailsSchema,
  projectSchema,
  projectViewSchema,
  portStatusSchema,
  startAllSchema,
  operationStartedSchema,
  serviceDetailsSchema,
  appUpdateSchema,
  backupRecordSchema,
  catalogCheckSchema,
  certificateSchema,
  databaseServiceSchema,
  diagnosticsPreviewSchema,
  restorePlanSchema,
  setupStatusSchema,
  updateSettingsSchema,
  uninstallPlanSchema,
  ngrokStatusSchema,
  tunnelViewSchema,
  certificateViewSchema,
  hostsPlanSchema,
  hostsViewSchema,
  siteCheckSchema,
  sitePlanSchema,
  siteViewSchema,
  serviceStatusSchema,
  serviceViewSchema,
  startupStatusSchema,
  launchAtLoginSchema,
  preferencesViewSchema,
  storageUsageSchema,
  terminalOpenedSchema,
  type Preferences,
  switchPreviewSchema,
  type Architecture,
  type CreateServiceRequest,
  type HostsChangeRequest,
  type SiteRequest,
  type ServiceStatus,
  type SwitchRequest,
  type OperationProgress,
  type RestoreRequest,
} from "../../types/dto";
import {
  jobEventSchema,
  jobPreviewSchema,
  jobRunSchema,
  jobViewSchema,
  type JobRequest,
} from "../../types/dto";
import { getIpcAdapter, type Unlisten } from "./adapter";
import { AppError, toAppError } from "./errors";

async function call<S extends z.ZodType>(
  command: string,
  args: Record<string, unknown> | undefined,
  schema: S,
): Promise<z.infer<S>> {
  let raw: unknown;
  try {
    raw = await getIpcAdapter().invoke(command, args);
  } catch (error) {
    throw toAppError(error);
  }
  const parsed = schema.safeParse(raw);
  if (!parsed.success) {
    throw new AppError({
      code: "INTERNAL",
      message: `Devluma's backend returned an unexpected response for "${command}".`,
      recoverable: true,
    });
  }
  return parsed.data;
}

function subscribe<S extends z.ZodType>(
  event: string,
  schema: S,
  handler: (payload: z.infer<S>) => void,
): Promise<Unlisten> {
  return getIpcAdapter().listen(event, (payload) => {
    const parsed = schema.safeParse(payload);
    if (parsed.success) handler(parsed.data);
    else console.warn(`Ignored malformed "${event}" event`);
  });
}

export const ipc = {
  getAppInfo: () => call("get_app_info", undefined, appInfoSchema),
  getStartupStatus: () => call("get_startup_status", undefined, startupStatusSchema),
  listOperations: () => call("list_operations", undefined, z.array(operationProgressSchema)),
  cancelOperation: (operationId: string) => call("cancel_operation", { operationId }, z.null()),
  listPackages: () => call("list_packages", undefined, catalogViewSchema),
  installPackage: (request: { packageId: string; version: string; architecture: Architecture }) =>
    call("install_package", { request }, installStartedSchema),
  previewRuntimeSwitch: (request: SwitchRequest) =>
    call("preview_runtime_switch", { request }, switchPreviewSchema),
  applyRuntimeSwitch: (request: SwitchRequest) =>
    call("apply_runtime_switch", { request }, operationProgressSchema),
  getPhpDetails: (installationId: string) => call("get_php_details", { installationId }, phpDetailsSchema),
  createPhpIni: (installationId: string, template: "development" | "production") =>
    call("create_php_ini", { installationId, template }, phpDetailsSchema),
  listProjects: () => call("list_projects", undefined, z.array(projectViewSchema)),
  /** `folder` is relative to the projects folder (`www`); it is created when missing. */
  createProject: (name: string, folder: string, serviceIds: string[]) =>
    call("create_project", { name, folder, serviceIds }, projectSchema),
  setProjectServices: (projectId: string, serviceIds: string[]) =>
    call("set_project_services", { projectId, serviceIds }, z.null()),
  startProjectServices: (projectId: string) => call("start_project_services", { projectId }, startAllSchema),
  removeProject: (projectId: string) => call("remove_project", { projectId }, z.null()),
  openProjectTerminal: (projectId: string) =>
    call("open_project_terminal", { projectId }, terminalOpenedSchema),
  openTerminal: () => call("open_terminal", undefined, terminalOpenedSchema),
  listServices: () => call("list_services", undefined, z.array(serviceViewSchema)),
  createService: (request: CreateServiceRequest) =>
    call("create_service", { request }, operationStartedSchema),
  startService: (serviceId: string) => call("start_service", { serviceId }, operationStartedSchema),
  stopService: (serviceId: string) => call("stop_service", { serviceId }, operationStartedSchema),
  restartService: (serviceId: string) => call("restart_service", { serviceId }, operationStartedSchema),
  getServiceDetails: (serviceId: string) => call("get_service_details", { serviceId }, serviceDetailsSchema),
  listSites: () => call("list_sites", undefined, z.array(siteViewSchema)),
  previewSite: (request: SiteRequest) => call("preview_site", { request }, sitePlanSchema),
  applySite: (request: SiteRequest) => call("apply_site", { request }, operationStartedSchema),
  removeSite: (siteId: string) => call("remove_site", { siteId }, operationStartedSchema),
  checkSite: (siteId: string) => call("check_site", { siteId }, siteCheckSchema),
  openServiceUi: (serviceId: string) => call("open_service_ui", { serviceId }, z.string()),
  openSite: (siteId: string) => call("open_site", { siteId }, z.string()),
  getSetupStatus: () => call("get_setup_status", undefined, setupStatusSchema),
  chooseStorage: (folder: string | null) =>
    call("choose_storage", { folder }, z.object({ restartNeeded: z.boolean() })),
  runSetup: (plan: { items: { packageId: string; version: string }[] }) =>
    call("run_setup", { plan }, z.object({ operationId: z.string() })),
  completeSetup: () => call("complete_setup", undefined, z.null()),
  restartApp: () => call("restart_app", undefined, z.null()),
  reopenSetup: () => call("reopen_setup", undefined, z.null()),
  getUpdateSettings: () => call("get_update_settings", undefined, updateSettingsSchema),
  setUpdateUrls: (appUrl: string, catalogUrl: string) =>
    call("set_update_urls", { appUrl, catalogUrl }, updateSettingsSchema),
  checkCatalogUpdate: () => call("check_catalog_update", undefined, catalogCheckSchema),
  checkAppUpdate: () => call("check_app_update", undefined, appUpdateSchema),
  installAppUpdate: () => call("install_app_update", undefined, z.null()),
  previewDiagnostics: (maskPaths: boolean) =>
    call("preview_diagnostics", { maskPaths }, diagnosticsPreviewSchema),
  exportDiagnostics: (maskPaths: boolean) => call("export_diagnostics", { maskPaths }, z.string()),
  openDiagnosticsFolder: () => call("open_diagnostics_folder", undefined, z.null()),
  previewUninstall: (installationId: string) =>
    call("preview_uninstall", { installationId }, uninstallPlanSchema),
  uninstallPackage: (installationId: string) => call("uninstall_package", { installationId }, z.unknown()),
  setServicePorts: (serviceId: string, port: number, adminPort: number | null) =>
    call("set_service_ports", { serviceId, port, adminPort }, z.unknown()),
  checkPort: (port: number) => call("check_port", { port }, portStatusSchema),
  listDatabaseServices: () => call("list_database_services", undefined, z.array(databaseServiceSchema)),
  listDatabases: (serviceId: string) => call("list_databases", { serviceId }, z.array(z.string())),
  listDatabaseBackups: () => call("list_database_backups", undefined, z.array(backupRecordSchema)),
  backupDatabase: (serviceId: string, database: string) =>
    call("backup_database", { serviceId, database }, backupRecordSchema),
  snapshotDatabase: (serviceId: string) => call("snapshot_database", { serviceId }, backupRecordSchema),
  previewRestore: (request: RestoreRequest) => call("preview_restore", { request }, restorePlanSchema),
  restoreDatabase: (request: RestoreRequest) => call("restore_database", { request }, restorePlanSchema),
  getNgrokStatus: () => call("get_ngrok_status", undefined, ngrokStatusSchema),
  setNgrokPath: (path: string) => call("set_ngrok_path", { path }, ngrokStatusSchema),
  setNgrokToken: (token: string) => call("set_ngrok_token", { token }, ngrokStatusSchema),
  removeNgrokToken: () => call("remove_ngrok_token", undefined, ngrokStatusSchema),
  listTunnels: () => call("list_tunnels", undefined, z.array(tunnelViewSchema)),
  startTunnel: (siteId: string) => call("start_tunnel", { siteId }, tunnelViewSchema),
  stopTunnel: (siteId: string) => call("stop_tunnel", { siteId }, z.null()),
  listCertificates: () => call("list_certificates", undefined, z.array(certificateViewSchema)),
  createLocalCa: () => call("create_local_ca", undefined, certificateSchema),
  createDevCertificate: (domains: string[]) => call("create_dev_certificate", { domains }, certificateSchema),
  trustLocalCa: () => call("trust_certificate", undefined, certificateSchema),
  removeLocalCaTrust: () => call("remove_certificate_trust", undefined, certificateSchema),
  renewCertificate: (certificateId: string) =>
    call("renew_certificate", { certificateId }, certificateSchema),
  deleteCertificate: (certificateId: string) => call("delete_certificate", { certificateId }, z.null()),
  getHosts: () => call("get_hosts", undefined, hostsViewSchema),
  previewHostsChange: (request: HostsChangeRequest) =>
    call("preview_hosts_change", { request }, hostsPlanSchema),
  applyHostsChange: (request: HostsChangeRequest) =>
    call("apply_hosts_change", { request }, operationStartedSchema),
  removeService: (serviceId: string) =>
    call("remove_service", { serviceId }, z.object({ keptDataPath: z.string().nullable() })),
  getPreferences: () => call("get_preferences", undefined, preferencesViewSchema),
  setPreferences: (preferences: Preferences) =>
    call("set_preferences", { preferences }, preferencesViewSchema),
  setLaunchAtLogin: (enabled: boolean) => call("set_launch_at_login", { enabled }, launchAtLoginSchema),
  getStorageUsage: () => call("get_storage_usage", undefined, storageUsageSchema),
  openStorageFolder: (key: string) => call("open_storage_folder", { key }, z.null()),
  // Scheduler (ADR 0017)
  listScheduledJobs: () => call("list_scheduled_jobs", undefined, z.array(jobViewSchema)),
  previewScheduledJob: (request: JobRequest) => call("preview_scheduled_job", { request }, jobPreviewSchema),
  saveScheduledJob: (request: JobRequest) => call("save_scheduled_job", { request }, jobViewSchema),
  setScheduledJobEnabled: (jobId: string, enabled: boolean) =>
    call("set_scheduled_job_enabled", { jobId, enabled }, jobViewSchema),
  runScheduledJobNow: (jobId: string) => call("run_scheduled_job_now", { jobId }, jobRunSchema),
  stopScheduledJobRun: (jobId: string) => call("stop_scheduled_job_run", { jobId }, z.null()),
  deleteScheduledJob: (jobId: string) => call("delete_scheduled_job", { jobId }, z.null()),
  listScheduledJobRuns: (jobId: string) => call("list_scheduled_job_runs", { jobId }, z.array(jobRunSchema)),
  getScheduledJobRunLog: (jobId: string, runId: string) =>
    call("get_scheduled_job_run_log", { jobId, runId }, z.string()),
};

export const events = {
  onOperationProgress: (handler: (progress: OperationProgress) => void) =>
    subscribe("operation://progress", operationProgressSchema, handler),
  onServiceState: (handler: (status: ServiceStatus) => void) =>
    subscribe("service://state", serviceStatusSchema, handler),
  onScheduledJob: (handler: (event: { jobId: string }) => void) =>
    subscribe("scheduler://job", jobEventSchema, handler),
};

export { AppError } from "./errors";
export { setNativeWindowTheme } from "./adapter";
export type { ErrorCode } from "./errors";
