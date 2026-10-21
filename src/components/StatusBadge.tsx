import {
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleHelp,
  CircleMinus,
  CircleStop,
  CirclePause,
  CircleSlash,
  Clock,
  CircleX,
  LoaderCircle,
  type LucideIcon,
} from "lucide-react";
import type { JobStatus, OperationStatus, RunStatus, ServiceState } from "../types/dto";

type Tone = "success" | "warning" | "danger" | "neutral" | "accent";

const TONES: Record<Tone, string> = {
  success: "bg-success-subtle text-success",
  warning: "bg-warning-subtle text-warning",
  danger: "bg-danger-subtle text-danger",
  neutral: "bg-neutral-subtle text-neutral",
  accent: "bg-accent-subtle text-accent",
};

interface StatusSpec {
  label: string;
  tone: Tone;
  icon: LucideIcon;
  spin?: boolean;
}

// Status is always icon + text, never color alone (docs/design-brief.md §7).
const SERVICE: Record<ServiceState, StatusSpec> = {
  not_installed: { label: "Not installed", tone: "neutral", icon: CircleDashed },
  stopped: { label: "Stopped", tone: "neutral", icon: CircleStop },
  starting: { label: "Starting", tone: "accent", icon: LoaderCircle, spin: true },
  running: { label: "Running", tone: "success", icon: CircleCheck },
  degraded: { label: "Degraded", tone: "warning", icon: CircleAlert },
  stopping: { label: "Stopping", tone: "accent", icon: LoaderCircle, spin: true },
  failed: { label: "Failed", tone: "danger", icon: CircleX },
  unknown: { label: "Unknown", tone: "neutral", icon: CircleHelp },
};

const OPERATION: Record<OperationStatus, StatusSpec> = {
  queued: { label: "Queued", tone: "neutral", icon: CircleDashed },
  running: { label: "Running", tone: "accent", icon: LoaderCircle, spin: true },
  succeeded: { label: "Complete", tone: "success", icon: CircleCheck },
  failed: { label: "Failed", tone: "danger", icon: CircleX },
  cancelled: { label: "Cancelled", tone: "neutral", icon: CircleMinus },
  needs_recovery: { label: "Needs recovery", tone: "warning", icon: CircleAlert },
};

function Badge({ spec }: { spec: StatusSpec }) {
  const Icon = spec.icon;
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium ${TONES[spec.tone]}`}
    >
      <Icon aria-hidden className={`size-3.5 ${spec.spin ? "motion-safe:animate-spin" : ""}`} />
      {spec.label}
    </span>
  );
}

export function ServiceStatusBadge({ state }: { state: ServiceState }) {
  return <Badge spec={SERVICE[state]} />;
}

export function OperationStatusBadge({ status }: { status: OperationStatus }) {
  return <Badge spec={OPERATION[status]} />;
}

// Scheduler (ADR 0017): the four job states the Scheduler page and status bar use.
const JOB: Record<JobStatus, StatusSpec> = {
  active: { label: "Active", tone: "success", icon: Clock },
  executing: { label: "Executing", tone: "accent", icon: LoaderCircle, spin: true },
  muted: { label: "Muted", tone: "neutral", icon: CirclePause },
  failed: { label: "Failed", tone: "danger", icon: CircleX },
};

const RUN: Record<RunStatus, StatusSpec> = {
  running: { label: "Running", tone: "accent", icon: LoaderCircle, spin: true },
  succeeded: { label: "Succeeded", tone: "success", icon: CircleCheck },
  failed: { label: "Failed", tone: "danger", icon: CircleX },
  timed_out: { label: "Timed out", tone: "danger", icon: CircleAlert },
  skipped: { label: "Skipped", tone: "neutral", icon: CircleSlash },
  stopped: { label: "Stopped", tone: "neutral", icon: CircleStop },
};

export function JobStatusBadge({ status }: { status: JobStatus }) {
  return <Badge spec={JOB[status]} />;
}

export function RunStatusBadge({ status }: { status: RunStatus }) {
  return <Badge spec={RUN[status]} />;
}
