import type { ProjectView, ServiceState, ServiceView, SiteView } from "../../types/dto";

/**
 * Stack map layout (DESIGN.md "Stack Map"). Each project runs as a line through the stack its
 * sites actually use: PHP → web server → domain. Only links the backend reports are drawn:
 * a site names its web server and PHP installation; a project without a site shows its
 * effective PHP and ends at "No site yet". Services no route uses (databases, Mailpit,
 * unused servers) go to the depot, because nothing links a project to them.
 */

export type StationKind = "php" | "web" | "domain";
/** Service states, plus `on_demand` (PHP with no FastCGI service yet) and `terminus`. */
export type StationState = ServiceState | "on_demand" | "terminus";

export interface Station {
  id: string;
  kind: StationKind;
  label: string;
  detail: string | null;
  serviceId: string | null;
  siteId: string | null;
  state: StationState;
  column: number;
  x: number;
  y: number;
  /** Route ids through this station, top to bottom. Two or more make an interchange. */
  routeIds: string[];
}

export interface Segment {
  d: string;
  /** Past a station that isn't running: drawn dashed ("delayed"). */
  delayed: boolean;
}

export interface Route {
  id: string;
  projectId: string;
  projectName: string;
  /** 1-based line ink (`--dl-line-N`), one per project. */
  ink: number;
  /** First route of its project carries the project label. */
  labelled: boolean;
  y: number;
  stationIds: string[];
  segments: Segment[];
  /** The station that delays this route, if any. */
  delayedAt: string | null;
}

export interface StackMapLayout {
  width: number;
  height: number;
  routes: Route[];
  stations: Station[];
  depot: ServiceView[];
}

export const INK_COUNT = 6;
export const ROW_HEIGHT = 76;
export const TRACK_GAP = 9;
const TOP = 56;
/** Vertical room above a station for its two-line label. */
const STATION_LABEL_SPACE = 44;
/** Width kept right of the domain column for its labels. */
const TERMINUS_LABEL_SPACE = 132;
const ORIGIN_X = 28;

const RUNNING_OK: ReadonlySet<StationState> = new Set(["running", "on_demand", "terminus"]);

export function isDelaying(state: StationState): boolean {
  return !RUNNING_OK.has(state);
}

interface RouteDraft {
  id: string;
  project: ProjectView;
  php: { installationId: string; version: string } | null;
  webServiceId: string | null;
  site: SiteView | null;
}

export function layoutStackMap(
  projects: ProjectView[],
  sites: SiteView[],
  services: ServiceView[],
  width: number,
): StackMapLayout {
  const byService = new Map(services.map((s) => [s.id, s]));
  const fastCgiByInstallation = new Map(
    services.filter((s) => s.role === "fast_cgi").map((s) => [s.packageInstallationId, s]),
  );

  const ordered = [...projects].sort((a, b) => a.name.localeCompare(b.name));
  const drafts: RouteDraft[] = [];
  for (const project of ordered) {
    const projectSites = sites
      .filter((s) => s.projectId === project.id)
      .sort((a, b) => a.domain.localeCompare(b.domain));
    if (projectSites.length === 0) {
      const effective = project.runtimes.find((r) => r.packageId === "php")?.effective ?? null;
      drafts.push({
        id: `${project.id}:none`,
        project,
        php: effective ? { installationId: effective.installationId, version: effective.version } : null,
        webServiceId: null,
        site: null,
      });
      continue;
    }
    for (const site of projectSites) {
      drafts.push({
        id: `${project.id}:${site.id}`,
        project,
        php:
          site.phpInstallationId && site.phpVersion
            ? { installationId: site.phpInstallationId, version: site.phpVersion }
            : null,
        webServiceId: site.webServerServiceId,
        site,
      });
    }
  }

  const w = Math.max(width, 420);
  // The domain column keeps room on its right for its labels (e.g. "admin.shop.test").
  const columnX = [Math.round(w * 0.32), Math.round(w * 0.56), w - TERMINUS_LABEL_SPACE];
  const stations = new Map<string, Station>();
  const rowY = (i: number) => TOP + i * ROW_HEIGHT;

  const addStation = (key: string, make: () => Omit<Station, "routeIds" | "y" | "x">, routeId: string) => {
    let st = stations.get(key);
    if (!st) {
      const base = make();
      st = { ...base, x: columnX[base.column] ?? 0, y: 0, routeIds: [] };
      stations.set(key, st);
    }
    st.routeIds.push(routeId);
    return key;
  };

  const inks = projectInks(projects);
  const routeStations = new Map<string, string[]>();
  drafts.forEach((r) => {
    const ids: string[] = [];
    if (r.php) {
      const php = r.php;
      ids.push(
        addStation(
          `php:${php.installationId}`,
          () => {
            const svc = fastCgiByInstallation.get(php.installationId);
            return {
              id: `php:${php.installationId}`,
              kind: "php",
              label: `PHP ${php.version}`,
              detail: svc?.port != null ? `:${svc.port}` : null,
              serviceId: svc?.id ?? null,
              siteId: null,
              state: svc ? svc.observedState : "on_demand",
              column: 0,
            };
          },
          r.id,
        ),
      );
    }
    if (r.webServiceId) {
      const webId = r.webServiceId;
      ids.push(
        addStation(
          `web:${webId}`,
          () => {
            const svc = byService.get(webId);
            const ports = [svc?.port, r.site?.httpsPort].filter((p): p is number => p != null);
            // The sign carries the server's name; version and ports go on the second line.
            return {
              id: `web:${webId}`,
              kind: "web",
              label: svc ? svc.packageName : (r.site?.webServerName ?? "Web server"),
              detail: [svc?.version, ...ports.map((p) => `:${p}`)].filter(Boolean).join(" ") || null,
              serviceId: svc?.id ?? null,
              siteId: null,
              state: svc ? svc.observedState : "unknown",
              column: 1,
            };
          },
          r.id,
        ),
      );
    }
    ids.push(
      addStation(
        r.site ? `site:${r.site.id}` : `none:${r.project.id}`,
        () => ({
          id: r.site ? `site:${r.site.id}` : `none:${r.project.id}`,
          kind: "domain",
          label: r.site ? r.site.domain : "No site yet",
          detail: r.site ? (r.site.sslEnabled ? "HTTPS" : "HTTP") : null,
          serviceId: null,
          siteId: r.site?.id ?? null,
          state: "terminus",
          column: 2,
        }),
        r.id,
      ),
    );
    routeStations.set(r.id, ids);
  });

  // Rows follow the stack, not the alphabet: lines sharing a PHP and web server sit together,
  // so the map crosses as little as possible.
  drafts.sort(
    (a, b) =>
      (b.php?.version ?? "").localeCompare(a.php?.version ?? "", undefined, { numeric: true }) ||
      (a.webServiceId ?? "￿").localeCompare(b.webServiceId ?? "￿") ||
      a.project.name.localeCompare(b.project.name) ||
      (a.site?.domain ?? "").localeCompare(b.site?.domain ?? ""),
  );
  const routeY = new Map(drafts.map((r, i) => [r.id, rowY(i)]));
  for (const st of stations.values()) {
    const ys = st.routeIds.map((id) => routeY.get(id) ?? 0);
    st.y = Math.round(ys.reduce((a, b) => a + b, 0) / ys.length);
  }
  // Stations sharing a column never overlap: push each one below the previous with room for
  // its label.
  for (let column = 0; column < 3; column++) {
    const inColumn = [...stations.values()].filter((s) => s.column === column).sort((a, b) => a.y - b.y);
    for (let i = 1; i < inColumn.length; i++) {
      const prev = inColumn[i - 1];
      const cur = inColumn[i];
      if (!prev || !cur) continue;
      const min = prev.y + stationHalfHeight(prev) + STATION_LABEL_SPACE + stationHalfHeight(cur);
      if (cur.y < min) cur.y = min;
    }
  }

  const trackY = (st: Station, routeId: string) => {
    const i = st.routeIds.indexOf(routeId);
    return st.y + (i - (st.routeIds.length - 1) / 2) * TRACK_GAP;
  };

  const seenProjects = new Set<string>();
  // A project's further sites branch off its first line instead of starting their own.
  const projectOrigin = new Map<string, number>();
  const routes: Route[] = drafts.map((r) => {
    const y = routeY.get(r.id) ?? 0;
    const ids = routeStations.get(r.id) ?? [];
    const originY = projectOrigin.get(r.project.id) ?? y;
    projectOrigin.set(r.project.id, originY);
    const points: [number, number][] = [[ORIGIN_X, originY]];
    for (const id of ids) {
      const st = stations.get(id);
      if (st) points.push([st.x, trackY(st, r.id)]);
    }
    const delayIndex = ids.findIndex((id) => {
      const st = stations.get(id);
      return st ? isDelaying(st.state) : false;
    });
    const segments: Segment[] = [];
    for (let i = 0; i < points.length - 1; i++) {
      const a = points[i];
      const b = points[i + 1];
      if (!a || !b) continue;
      // Segment i runs into station i; it is delayed once it leaves the delaying station.
      segments.push({ d: route45(a, b), delayed: delayIndex !== -1 && i > delayIndex });
    }
    const labelled = !seenProjects.has(r.project.id);
    seenProjects.add(r.project.id);
    return {
      id: r.id,
      projectId: r.project.id,
      projectName: r.project.name,
      ink: inks.get(r.project.id) ?? 1,
      labelled,
      y: originY,
      stationIds: ids,
      segments,
      delayedAt: delayIndex === -1 ? null : (ids[delayIndex] ?? null),
    };
  });

  const stationList = [...stations.values()].sort((a, b) => a.column - b.column || a.y - b.y);
  const usedServices = new Set(
    [...stations.values()].map((s) => s.serviceId).filter((id): id is string => id !== null),
  );
  const depot = services
    .filter((s) => !usedServices.has(s.id))
    .sort((a, b) => a.instanceName.localeCompare(b.instanceName));

  const lowest = Math.max(
    0,
    ...stationList.map((s) => s.y + stationHalfHeight(s)),
    ...routes.map((r) => r.y),
  );
  return {
    width: w,
    height: lowest + 40,
    routes,
    stations: stationList,
    depot,
  };
}

/** Each project's line ink (1-based `--dl-line-N`), by name, so every view agrees. */
export function projectInks(projects: Pick<ProjectView, "id" | "name">[]): Map<string, number> {
  return new Map(
    [...projects].sort((a, b) => a.name.localeCompare(b.name)).map((p, i) => [p.id, (i % INK_COUNT) + 1]),
  );
}

/** Half the drawn height of a station (interchanges grow with their tracks). */
export function stationHalfHeight(st: Pick<Station, "kind" | "routeIds">): number {
  if (st.kind === "domain") return 13;
  return Math.max(20, (st.routeIds.length - 1) * TRACK_GAP + 20) / 2;
}

/** Short straight run into a station after a bend. */
export const APPROACH = 20;

/**
 * Leave a station level, take one 45° diagonal just before the next station, arrive level.
 * Lines therefore run flat beside every station, which keeps the signs above them clear.
 */
export function route45([ax, ay]: [number, number], [bx, by]: [number, number]): string {
  const dy = by - ay;
  if (dy === 0) return `M${ax} ${ay}H${bx}`;
  const run = Math.min(Math.abs(dy), Math.max(bx - ax - APPROACH - 12, 0));
  const x2 = bx - APPROACH;
  const x1 = x2 - run;
  return `M${ax} ${ay}H${x1}L${x2} ${by}H${bx}`;
}
