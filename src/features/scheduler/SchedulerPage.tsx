import { CalendarClock, Plus } from "lucide-react";
import { Fragment, useEffect, useState, type ReactNode } from "react";
import { Link, NavLink, useNavigate, useParams } from "react-router";
import { Commands } from "../../app/commandBar";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
import { PageHeader } from "../../components/PageHeader";
import { JobStatusBadge, RunStatusBadge } from "../../components/StatusBadge";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { JobRun, JobView } from "../../types/dto";
import { JobForm } from "./JobForm";
import { useScheduledJobs } from "./useScheduledJobs";

type Mode = "list" | "new" | "detail" | "edit";

const KIND_LABELS: Record<JobView["kind"], string> = {
  artisan_schedule: "Laravel scheduler",
  php_script: "PHP script",
  command: "Command",
};

function when(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? iso
    : d.toLocaleString(undefined, {
        weekday: "short",
        day: "numeric",
        month: "short",
        hour: "2-digit",
        minute: "2-digit",
      });
}

function duration(run: JobRun): string {
  if (!run.finishedAt) return "—";
  const ms = new Date(run.finishedAt).getTime() - new Date(run.startedAt).getTime();
  if (!Number.isFinite(ms) || ms < 0) return "—";
  return ms < 1000 ? "<1 s" : ms < 60_000 ? `${Math.round(ms / 1000)} s` : `${Math.round(ms / 60_000)} min`;
}

/** Scheduler (ADR 0017): jobs on the left, the selected job (or the form) on the right. */
export function SchedulerPage({ mode }: { mode: Mode }) {
  const { jobId } = useParams();
  const jobs = useScheduledJobs();
  const navigate = useNavigate();
  const job = jobs.status === "success" ? jobs.data.find((j) => j.id === jobId) : undefined;

  let pane: ReactNode;
  if (jobs.status === "pending")
    pane = <LoadingState label="Loading scheduled jobs…" shape="panel" rows={4} />;
  else if (jobs.status === "error") pane = <ErrorPanel error={jobs.error} onRetry={jobs.reload} />;
  else if (mode === "new") pane = <NewJob onSaved={(j) => navigate(`/scheduler/${j.id}`)} />;
  else if ((mode === "detail" || mode === "edit") && !job)
    pane = <PageHeader title="Job not found" description="It may have been deleted." />;
  else if (mode === "edit" && job)
    pane = <EditJob job={job} onDone={() => navigate(`/scheduler/${job.id}`)} />;
  else if (mode === "detail" && job) pane = <JobDetail job={job} />;
  else pane = <Overview jobs={jobs.data} />;

  return (
    <div className="flex min-h-0 flex-1">
      <nav
        aria-label="Scheduled jobs"
        className="hidden w-72 shrink-0 flex-col gap-4 overflow-y-auto border-r border-border bg-surface-raised py-5 wide:flex"
      >
        <h2 className="label-caps px-5 text-fg-muted">Scheduled jobs</h2>
        {jobs.status === "success" && <JobList jobs={jobs.data} />}
      </nav>
      <div className="flex min-w-0 flex-1 flex-col gap-5 overflow-y-auto px-7 py-6">{pane}</div>
    </div>
  );
}

function JobList({ jobs }: { jobs: JobView[] }) {
  const projects = [...new Set(jobs.map((j) => j.projectName))].sort((a, b) => a.localeCompare(b));
  if (jobs.length === 0) return <p className="px-5 text-xs text-fg-muted">No jobs yet.</p>;
  return (
    <>
      {projects.map((p) => (
        <div key={p} className="flex flex-col gap-0.5 px-2">
          <span className="px-3 pb-1 text-xs font-semibold text-fg-muted">{p}</span>
          {jobs
            .filter((j) => j.projectName === p)
            .map((j) => (
              <NavLink
                key={j.id}
                to={`/scheduler/${j.id}`}
                className={({ isActive }) =>
                  `relative flex items-center justify-between gap-2 rounded-md px-3 py-2 text-sm transition-colors ${
                    isActive
                      ? "bg-surface font-semibold before:absolute before:top-2 before:bottom-2 before:left-0 before:w-[3px] before:rounded-full before:bg-marker"
                      : "hover:bg-surface-sunken"
                  }`
                }
              >
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className="truncate">{j.name}</span>
                  <span className="truncate text-xs font-normal text-fg-muted">{j.scheduleText}</span>
                </span>
                <JobStatusBadge status={j.status} />
              </NavLink>
            ))}
        </div>
      ))}
    </>
  );
}

function NewJobLink({ primary = false }: { primary?: boolean }) {
  return (
    <Link
      to="/scheduler/new"
      className={`inline-flex min-h-8 items-center gap-2 rounded border px-3 text-sm font-medium transition-colors ${
        primary
          ? "border-transparent bg-accent text-accent-fg hover:bg-accent-hover"
          : "border-border-strong/70 bg-surface-raised hover:bg-surface-sunken"
      }`}
    >
      <Plus aria-hidden className="size-4" />
      New job
    </Link>
  );
}

function Overview({ jobs }: { jobs: JobView[] }) {
  return (
    <>
      <PageHeader
        title="Scheduler"
        description="Recurring project tasks (Laravel's scheduler, PHP scripts or commands) that Devluma runs while it's open."
      />
      <Commands>
        <NewJobLink primary />
      </Commands>
      {jobs.length === 0 ? (
        <EmptyState
          icon={CalendarClock}
          title="No scheduled jobs yet"
          description="Run php artisan schedule:run every minute for Laravel (Pulse, queues, reports), or any script on a schedule, without setting up Windows Task Scheduler."
          action={<NewJobLink primary />}
        />
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Scheduled jobs</caption>
            <thead className="bg-surface-raised">
              <tr>
                {["Job", "Project", "Schedule", "Next run", "Status"].map((h) => (
                  <th key={h} scope="col" className="px-3 py-2 font-semibold">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {jobs.map((j) => (
                <tr key={j.id}>
                  <th scope="row" className="px-3 py-2 font-medium">
                    <Link to={`/scheduler/${j.id}`} className="text-accent hover:underline">
                      {j.name}
                    </Link>
                  </th>
                  <td className="px-3 py-2">{j.projectName}</td>
                  <td className="px-3 py-2">{j.scheduleText}</td>
                  <td className="px-3 py-2 whitespace-nowrap">{j.enabled ? when(j.nextRun) : "—"}</td>
                  <td className="px-3 py-2">
                    <JobStatusBadge status={j.status} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function NewJob({ onSaved }: { onSaved: (job: JobView) => void }) {
  const projects = useAsync(ipc.listProjects);
  const navigate = useNavigate();
  return (
    <>
      <PageHeader
        title="New job"
        description="Choose what to run and when. You'll see exactly what will run before it's saved."
      />
      {projects.status === "pending" && <LoadingState label="Loading projects…" shape="inline" />}
      {projects.status === "error" && <ErrorPanel error={projects.error} onRetry={projects.reload} />}
      {projects.status === "success" &&
        (projects.data.length === 0 ? (
          <EmptyState
            icon={CalendarClock}
            title="Add a project first"
            description="Jobs run inside a project folder with that project's PHP, Node and Python."
            action={
              <Link to="/projects" className="text-sm font-medium text-accent hover:underline">
                Go to Projects
              </Link>
            }
          />
        ) : (
          <JobForm projects={projects.data} onSaved={onSaved} onCancel={() => navigate("/scheduler")} />
        ))}
    </>
  );
}

function EditJob({ job, onDone }: { job: JobView; onDone: () => void }) {
  const projects = useAsync(ipc.listProjects);
  return (
    <>
      <PageHeader title={`Edit ${job.name}`} />
      {projects.status === "pending" && <LoadingState label="Loading projects…" shape="inline" />}
      {projects.status === "error" && <ErrorPanel error={projects.error} onRetry={projects.reload} />}
      {projects.status === "success" && (
        <JobForm job={job} projects={projects.data} onSaved={onDone} onCancel={onDone} />
      )}
    </>
  );
}

function JobDetail({ job }: { job: JobView }) {
  const navigate = useNavigate();
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [deleting, setDeleting] = useState(false);
  const executing = job.status === "executing";

  const act = (label: string, fn: () => Promise<unknown>) => {
    setPending(label);
    setError(null);
    fn().then(
      () => setPending(null),
      (e: unknown) => {
        setPending(null);
        setError(toAppError(e));
      },
    );
  };

  return (
    <>
      <PageHeader
        title={job.name}
        description={`${job.projectName} · ${KIND_LABELS[job.kind]} · ${job.scheduleText}`}
      />
      <Commands>
        {executing ? (
          <Button
            pending={pending === "stop"}
            onClick={() => act("stop", () => ipc.stopScheduledJobRun(job.id))}
          >
            Stop
          </Button>
        ) : (
          <Button
            variant="primary"
            pending={pending === "run"}
            disabled={pending !== null || job.problem !== null}
            onClick={() => act("run", () => ipc.runScheduledJobNow(job.id))}
          >
            Run now
          </Button>
        )}
        <Button
          pending={pending === "mute"}
          disabled={pending !== null}
          onClick={() => act("mute", () => ipc.setScheduledJobEnabled(job.id, !job.enabled))}
        >
          {job.enabled ? "Mute" : "Unmute"}
        </Button>
        <Link
          to={`/scheduler/${job.id}/edit`}
          className="inline-flex min-h-8 items-center rounded border border-border-strong/70 bg-surface-raised px-3 text-sm font-medium hover:bg-surface-sunken"
        >
          Edit
        </Link>
        <Button variant="ghost" onClick={() => setDeleting(true)}>
          Delete
        </Button>
        <NewJobLink />
      </Commands>

      <div className="flex flex-wrap items-center gap-3 text-sm">
        <JobStatusBadge status={job.status} />
        <span className="text-fg-muted">
          {executing
            ? `Running since ${when(job.runningSince)}`
            : job.enabled
              ? `Next run ${when(job.nextRun)}`
              : "Muted: it won't run until you unmute it."}
          {job.lastRun && !executing && ` · Last run ${when(job.lastRun.startedAt)}`}
        </span>
      </div>
      {job.status === "failed" && job.lastRun?.message && (
        <p className="text-sm text-danger">{job.lastRun.message}</p>
      )}
      {job.problem && (
        <section
          aria-label="Job problem"
          className="rounded-md border border-warning/40 bg-warning-subtle p-4 text-sm"
        >
          This job can&apos;t run right now: {job.problem}
        </section>
      )}
      {error && <ErrorPanel error={error} />}

      {job.command && (
        <section aria-labelledby="command-heading" className="flex flex-col gap-2">
          <h2 id="command-heading" className="label-caps text-fg-muted">
            What runs
          </h2>
          <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-2 rounded-md border border-border p-4 text-sm">
            <dt className="text-fg-muted">Command</dt>
            <dd className="flex min-w-0 items-start gap-1">
              <span className="min-w-0 font-mono text-xs break-all select-all">
                {[job.command.executable, ...job.command.args].join(" ")}
              </span>
              <CopyButton value={[job.command.executable, ...job.command.args].join(" ")} label="command" />
            </dd>
            <dt className="text-fg-muted">Folder</dt>
            <dd className="font-mono text-xs break-all">{job.command.workingDir}</dd>
            <dt className="text-fg-muted">PATH additions</dt>
            <dd className="font-mono text-xs break-all">
              {job.command.pathPrefix.length ? job.command.pathPrefix.join("; ") : "None"}
            </dd>
            <dt className="text-fg-muted">Time limit</dt>
            <dd>{Math.round(job.timeoutSecs / 60)} minutes</dd>
            <dt className="text-fg-muted">Schedule</dt>
            <dd>
              {job.scheduleText} <span className="font-mono text-xs text-fg-muted">({job.cron})</span>
            </dd>
          </dl>
        </section>
      )}

      <RunHistory job={job} />

      {deleting && (
        <ConfirmChangeDialog
          open
          destructive
          onOpenChange={(open) => !open && setDeleting(false)}
          title={`Delete ${job.name}?`}
          target={job.command ? [job.command.executable, ...job.command.args].join(" ") : job.name}
          impact={
            <>
              The job stops running and its run history and logs are deleted.
              {executing && " Its current run is stopped first."} The project&apos;s files are not touched.
            </>
          }
          confirmLabel="Delete job"
          onConfirm={async () => {
            await ipc.deleteScheduledJob(job.id);
            navigate("/scheduler");
          }}
        />
      )}
    </>
  );
}

function RunHistory({ job }: { job: JobView }) {
  const runs = useAsync(() => ipc.listScheduledJobRuns(job.id));
  const [open, setOpen] = useState<string | null>(null);
  const { refresh } = runs;
  // A run started or ended (the job view changes with it): reload the history.
  const signature = `${job.status}:${job.lastRun?.id ?? ""}:${job.runningSince ?? ""}`;
  useEffect(() => {
    refresh();
  }, [signature, refresh]);

  return (
    <section aria-labelledby="runs-heading" className="flex flex-col gap-2">
      <h2 id="runs-heading" className="label-caps text-fg-muted">
        Run history
      </h2>
      {runs.status === "pending" && <LoadingState label="Loading runs…" shape="table" columns={5} />}
      {runs.status === "error" && <ErrorPanel error={runs.error} onRetry={runs.reload} />}
      {runs.status === "success" &&
        (runs.data.length === 0 ? (
          <p className="text-sm text-fg-muted">No runs yet.</p>
        ) : (
          <div className="overflow-x-auto rounded-md border border-border">
            <table className="w-full text-left text-sm">
              <caption className="sr-only">Runs of {job.name}, newest first</caption>
              <thead className="bg-surface-raised">
                <tr>
                  {["Started", "Trigger", "Result", "Exit code", "Took", ""].map((h, i) => (
                    <th key={i} scope="col" className="px-3 py-2 font-semibold">
                      {h || <span className="sr-only">Log</span>}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {runs.data.map((r) => (
                  <Fragment key={r.id}>
                    <tr className="align-top">
                      <td className="px-3 py-2 whitespace-nowrap">{when(r.startedAt)}</td>
                      <td className="px-3 py-2">{r.trigger === "manual" ? "Run now" : "Schedule"}</td>
                      <td className="px-3 py-2">
                        <RunStatusBadge status={r.status} />
                        {r.message && r.status !== "succeeded" && (
                          <p className="mt-1 max-w-md text-xs text-fg-muted">{r.message}</p>
                        )}
                      </td>
                      <td className="px-3 py-2 font-mono">{r.exitCode ?? "—"}</td>
                      <td className="px-3 py-2 whitespace-nowrap">{duration(r)}</td>
                      <td className="px-3 py-2">
                        {r.status !== "skipped" && (
                          <Button
                            variant="ghost"
                            aria-expanded={open === r.id}
                            onClick={() => setOpen((o) => (o === r.id ? null : r.id))}
                          >
                            {open === r.id ? "Hide log" : "Log"}
                          </Button>
                        )}
                      </td>
                    </tr>
                    {open === r.id && (
                      <tr>
                        <td colSpan={6} className="px-3 pb-3">
                          <RunLog jobId={job.id} runId={r.id} />
                        </td>
                      </tr>
                    )}
                  </Fragment>
                ))}
              </tbody>
            </table>
          </div>
        ))}
    </section>
  );
}

function RunLog({ jobId, runId }: { jobId: string; runId: string }) {
  const log = useAsync(() => ipc.getScheduledJobRunLog(jobId, runId));
  if (log.status === "pending") return <LoadingState label="Loading the log…" shape="inline" />;
  if (log.status === "error") return <ErrorPanel error={log.error} onRetry={log.reload} />;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between text-xs text-fg-muted">
        <span>Output (secrets are redacted).</span>
        <CopyButton value={log.data} label="log" />
      </div>
      <pre
        aria-label="Run log"
        className="max-h-80 overflow-auto rounded-md bg-surface-sunken p-3 font-mono text-xs whitespace-pre-wrap"
      >
        {log.data || "No output."}
      </pre>
    </div>
  );
}
