import { Navigate, type RouteObject } from "react-router";
import { CertificatesPage } from "../features/certificates/CertificatesPage";
import { DatabasesPage } from "../features/databases/DatabasesPage";
import { HostsPage } from "../features/hosts/HostsPage";
import { LogsPage } from "../features/logs/LogsPage";
import { OverviewPage } from "../features/overview/OverviewPage";
import { PlannedPage } from "../features/planned/PlannedPage";
import { PLANNED_PAGES } from "../features/planned/plannedPages";
import { ProjectsPage } from "../features/projects/ProjectsPage";
import { PackageDetailPage } from "../features/runtimes/PackageDetailPage";
import { PhpInstallationPage } from "../features/runtimes/PhpInstallationPage";
import { RuntimesPage } from "../features/runtimes/RuntimesPage";
import { ServiceDetailPage } from "../features/services/ServiceDetailPage";
import { ServicesPage } from "../features/services/ServicesPage";
import { SettingsPage } from "../features/settings/SettingsPage";
import { SitesPage } from "../features/sites/SitesPage";
import { AppShell } from "./AppShell";

export const routes: RouteObject[] = [
  {
    path: "/",
    element: <AppShell />,
    children: [
      { index: true, element: <OverviewPage /> },
      ...Object.entries(PLANNED_PAGES).map(([path, props]) => ({
        path: path.slice(1),
        element: <PlannedPage {...props} />,
      })),
      { path: "projects", element: <ProjectsPage /> },
      { path: "services", element: <ServicesPage /> },
      { path: "services/:serviceId", element: <ServiceDetailPage /> },
      { path: "runtimes", element: <RuntimesPage /> },
      { path: "runtimes/:packageId", element: <PackageDetailPage /> },
      { path: "runtimes/php/installations/:installationId", element: <PhpInstallationPage /> },
      { path: "sites", element: <SitesPage /> },
      { path: "hosts", element: <HostsPage /> },
      { path: "ssl", element: <CertificatesPage /> },
      { path: "databases", element: <DatabasesPage /> },
      { path: "logs", element: <LogsPage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "*", element: <Navigate to="/" replace /> },
    ],
  },
];
