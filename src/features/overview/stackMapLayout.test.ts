import { describe, expect, it } from "vitest";
import type { ProjectView, ServiceView, SiteView } from "../../types/dto";
import { layoutStackMap, route45 } from "./stackMapLayout";

function project(id: string, name: string, php?: { installationId: string; version: string }): ProjectView {
  return {
    id,
    name,
    rootPath: String.raw`C:\Projects\${name}`,
    projectType: "laravel",
    localDomain: null,
    notes: null,
    isFavorite: false,
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
    lastOpenedAt: null,
    detection: {
      kind: "laravel",
      label: "Laravel",
      suggestedDocumentRoot: "public",
      virtualEnvScripts: null,
    },
    runtimes: php
      ? [
          {
            packageId: "php",
            packageName: "PHP",
            effective: { ...php, installPath: String.raw`C:\Devluma\packages\php`, source: "global_default" },
          },
        ]
      : [],
  };
}

function service(id: string, over: Partial<ServiceView>): ServiceView {
  return {
    id,
    packageInstallationId: `inst-${id}`,
    instanceName: id,
    serviceType: "web_server",
    lifecycleAdapter: "nginx",
    desiredState: "running",
    observedState: "running",
    port: 80,
    bindAddress: "127.0.0.1",
    configPath: null,
    dataPath: null,
    pid: 100,
    processStartedAt: null,
    healthStatus: "healthy",
    lastExitCode: null,
    lastErrorCode: null,
    jobName: null,
    adminPort: null,
    updatedAt: "2026-10-01T00:00:00Z",
    role: "web_server",
    packageId: "nginx",
    packageName: "Nginx",
    version: "1.27.4",
    ...over,
  };
}

function site(id: string, projectId: string, domain: string, web: string, php: string | null): SiteView {
  return {
    id,
    projectId,
    domain,
    documentRoot: String.raw`C:\Projects\x\public`,
    webServerServiceId: web,
    phpInstallationId: php,
    httpPort: 80,
    httpsPort: null,
    sslEnabled: false,
    configPath: "x.conf",
    configHash: null,
    status: "active",
    hostsManaged: true,
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
    projectName: projectId,
    webServerName: "Nginx",
    webServerRunning: true,
    url: `http://${domain}`,
    httpsUrl: null,
    phpVersion: php ? "8.3.14" : null,
    phpOutdated: false,
    configPresent: true,
  };
}

const nginx = service("nginx", {});
const phpCgi = service("phpcgi", {
  role: "fast_cgi",
  packageInstallationId: "php83",
  packageName: "PHP",
  version: "8.3.14",
  port: 9083,
});
const maria = service("maria", { role: "database", packageName: "MariaDB", version: "11.4.5", port: 3306 });

describe("layoutStackMap", () => {
  it("draws one line per site through its PHP, web server and domain, sharing stations", () => {
    const layout = layoutStackMap(
      [project("blog", "blog"), project("shop", "shop")],
      [site("s1", "blog", "blog.test", "nginx", "php83"), site("s2", "shop", "shop.test", "nginx", "php83")],
      [nginx, phpCgi, maria],
      800,
    );
    expect(layout.routes.map((r) => r.projectName)).toEqual(["blog", "shop"]);
    expect(layout.routes.map((r) => r.ink)).toEqual([1, 2]);
    const web = layout.stations.find((s) => s.id === "web:nginx");
    expect(web?.routeIds).toHaveLength(2);
    expect(layout.stations.find((s) => s.id === "php:php83")?.state).toBe("running");
    expect(layout.routes.every((r) => r.delayedAt === null)).toBe(true);
    expect(layout.routes.every((r) => r.segments.every((s) => !s.delayed))).toBe(true);
  });

  it("delays every line past a station that is not running", () => {
    const layout = layoutStackMap(
      [project("blog", "blog")],
      [site("s1", "blog", "blog.test", "nginx", "php83")],
      [{ ...nginx, observedState: "degraded" }, phpCgi],
      800,
    );
    const route = layout.routes[0];
    expect(route?.delayedAt).toBe("web:nginx");
    // origin→PHP, PHP→web are clear; web→domain is held up.
    expect(route?.segments.map((s) => s.delayed)).toEqual([false, false, true]);
  });

  it("keeps services no project links to in the depot, never on a line", () => {
    const layout = layoutStackMap(
      [project("blog", "blog")],
      [site("s1", "blog", "blog.test", "nginx", "php83")],
      [nginx, phpCgi, maria],
      800,
    );
    expect(layout.depot.map((s) => s.id)).toEqual(["maria"]);
    expect(layout.stations.some((s) => s.serviceId === "maria")).toBe(false);
  });

  it("ends a project without a site at 'No site yet', via its effective PHP", () => {
    const layout = layoutStackMap(
      [project("api", "api", { installationId: "php82", version: "8.2.27" })],
      [],
      [],
      800,
    );
    const [route] = layout.routes;
    expect(route?.stationIds).toEqual(["php:php82", "none:api"]);
    expect(layout.stations.find((s) => s.id === "php:php82")?.state).toBe("on_demand");
    expect(layout.stations.find((s) => s.id === "none:api")?.label).toBe("No site yet");
  });

  it("returns an empty map when there are no projects", () => {
    const layout = layoutStackMap([], [], [nginx], 800);
    expect(layout.routes).toEqual([]);
    expect(layout.depot).toEqual([nginx]);
  });
});

describe("route45", () => {
  it("bends only through a 45° diagonal", () => {
    expect(route45([0, 10], [100, 10])).toBe("M0 10H100");
    // Level out of the first station, bend just before the second, arrive level.
    expect(route45([0, 0], [100, 20])).toBe("M0 0H60L80 20H100");
  });
});
