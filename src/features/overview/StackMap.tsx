import type { CSSProperties } from "react";
import {
  isDelaying,
  stationHalfHeight,
  type StackMapLayout,
  type Station,
  type StationState,
} from "./stackMapLayout";

export const STATE_LABELS: Record<StationState, string> = {
  not_installed: "Not installed",
  stopped: "Stopped",
  starting: "Starting",
  running: "Running",
  degraded: "Degraded",
  stopping: "Stopping",
  failed: "Failed",
  unknown: "Unknown",
  on_demand: "No FastCGI service yet",
  terminus: "Site",
};

const KIND_LABELS = { php: "PHP", web: "Web server", domain: "Site" } as const;

/** Ring colour around a station that holds up its lines. */
function ringColor(state: StationState): string | null {
  if (state === "failed") return "var(--dl-danger)";
  if (state === "degraded" || state === "unknown" || state === "starting" || state === "stopping")
    return "var(--dl-delay)";
  if (state === "stopped" || state === "not_installed") return "var(--dl-border-strong)";
  return null;
}

export function inkVar(ink: number): string {
  return `var(--dl-line-${ink})`;
}

interface StackMapProps {
  layout: StackMapLayout;
  /** A station id, or `proj:<id>` for a project line. */
  selectedId: string | null;
  onSelect: (id: string) => void;
}

const LABEL_HALO = { textShadow: "0 0 3px var(--dl-map), 0 0 3px var(--dl-map), 0 0 2px var(--dl-map)" };

export function StackMap({ layout, selectedId, onSelect }: StackMapProps) {
  const lineCount = new Set(layout.routes.map((r) => r.projectId)).size;
  return (
    <div
      role="group"
      aria-label={`Stack map: ${lineCount} project${lineCount === 1 ? "" : "s"}`}
      className="relative"
      style={{ width: layout.width, height: layout.height }}
    >
      <svg aria-hidden className="absolute inset-0" width={layout.width} height={layout.height}>
        {layout.routes.map((r) => (
          <g key={r.id} stroke={inkVar(r.ink)} fill="none" strokeLinecap="round" strokeLinejoin="round">
            {r.segments.map((seg, i) => (
              <path
                key={i}
                d={seg.d}
                strokeWidth={6}
                strokeDasharray={seg.delayed ? "2 11" : undefined}
                opacity={seg.delayed ? 0.75 : 1}
                className={seg.delayed ? "delay-crawl" : undefined}
              />
            ))}
            <circle cx={28} cy={r.y} r={5} fill={inkVar(r.ink)} stroke="none" />
          </g>
        ))}
      </svg>

      {layout.routes
        .filter((r) => r.labelled)
        .map((r) => {
          const id = `proj:${r.projectId}`;
          return (
            <button
              key={r.id}
              type="button"
              onClick={() => onSelect(id)}
              aria-pressed={selectedId === id}
              aria-label={`Project ${r.projectName}`}
              className={`label-caps absolute rounded-sm px-1 whitespace-nowrap text-fg transition-colors hover:text-accent ${
                selectedId === id ? "underline decoration-2 underline-offset-4" : ""
              }`}
              style={{ left: 18, top: r.y - 30, textDecorationColor: inkVar(r.ink), ...LABEL_HALO }}
            >
              {r.projectName}
            </button>
          );
        })}

      {layout.stations.map((st) => (
        <StationButton
          key={st.id}
          maxLabelWidth={labelRoom(layout, st)}
          mapHeight={layout.height}
          station={st}
          selected={st.id === selectedId}
          onSelect={() => onSelect(st.id)}
        />
      ))}
    </div>
  );
}

function StateText({ state }: { state: StationState }) {
  return (
    <span
      className="text-xs font-semibold whitespace-nowrap"
      style={{ color: state === "failed" ? "var(--dl-danger)" : "var(--dl-warning)" }}
    >
      {STATE_LABELS[state]}
    </span>
  );
}

/** Room for a station's label before the next column's labels begin. */
function labelRoom(layout: StackMapLayout, st: Station): number {
  const next = layout.stations.find((s) => s.column === st.column + 1);
  return next ? Math.max(next.x - st.x - 20, 72) : layout.width - st.x;
}

function StationButton({
  station: st,
  selected,
  onSelect,
  maxLabelWidth,
  mapHeight,
}: {
  station: Station;
  selected: boolean;
  onSelect: () => void;
  maxLabelWidth: number;
  mapHeight: number;
}) {
  const lines = st.routeIds.length;
  const terminus = st.kind === "domain";
  const w = terminus ? 12 : 20;
  const h = stationHalfHeight(st) * 2;
  const ring = ringColor(st.state);
  const delaying = isDelaying(st.state);
  const shadow = [
    ring ? `0 0 0 3px var(--dl-map), 0 0 0 6px ${ring}` : null,
    selected ? `0 0 0 ${ring ? 9 : 3}px var(--dl-map), 0 0 0 ${ring ? 11 : 5}px var(--dl-focus)` : null,
  ]
    .filter(Boolean)
    .join(", ");
  const style: CSSProperties = {
    left: st.x - w / 2,
    top: st.y - h / 2,
    width: w,
    height: h,
    boxShadow: shadow || undefined,
  };
  const stateText = terminus ? null : STATE_LABELS[st.state];
  // Outermost ring (delay ring, then selection ring) plus a gap, so no sign touches a ring.
  const clearance = (selected ? (ring ? 11 : 5) : ring ? 6 : 0) + 6;
  const signBelow = st.kind === "web";
  return (
    <>
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={selected}
        aria-label={`${KIND_LABELS[st.kind]} ${st.label}${stateText ? `, ${stateText}` : ""}${
          lines > 1 ? `, shared by ${lines} lines` : ""
        }`}
        className="absolute rounded-full border-[3px] border-station-ring bg-station transition-transform hover:scale-110 focus-visible:outline-offset-4 motion-reduce:hover:scale-100"
        style={style}
      />
      <span
        aria-hidden
        // Signs keep clear of the station's outermost ring. Web servers sign below the station
        // (their lines leave level and bend late, toward the domains, so the space under them
        // is clear); other stations sign above, anchored by the bottom edge so a long name wraps
        // upward instead of being cut.
        className="pointer-events-none absolute flex flex-col leading-tight"
        style={{
          left: st.x - 6,
          ...(signBelow
            ? { top: st.y + h / 2 + clearance }
            : { bottom: mapHeight - (st.y - h / 2) + clearance }),
          maxWidth: maxLabelWidth,
          ...LABEL_HALO,
        }}
      >
        {/* Station names are signs (tracked caps); domains stay literal — people type them. */}
        <span className={`break-words ${terminus ? "text-[0.8125rem] font-semibold" : "label-caps"}`}>
          {st.label}
        </span>
        <span className="font-mono text-[0.6875rem] break-words text-fg-muted">{st.detail}</span>
        {signBelow && delaying && <StateText state={st.state} />}
      </span>
      {!signBelow && delaying && (
        // The state reads in full under the station, never truncated with the sign.
        <span
          aria-hidden
          className="pointer-events-none absolute"
          style={{ left: st.x - 12, top: st.y + h / 2 + clearance, ...LABEL_HALO }}
        >
          <StateText state={st.state} />
        </span>
      )}
    </>
  );
}
