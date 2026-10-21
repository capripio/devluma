import { useState, type FormEvent, type ReactNode } from "react";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { ErrorPanel } from "../../components/ErrorPanel";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import type { JobKind, JobPreview, JobRequest, JobView, ProjectView } from "../../types/dto";
import { DEFAULT_CHOICE, PRESETS, WEEKDAYS, fromCron, toCron, type ScheduleChoice } from "./schedule";

const KINDS: { value: JobKind; label: string; hint: string }[] = [
  {
    value: "artisan_schedule",
    label: "Laravel scheduler",
    hint: "Runs php artisan schedule:run with the project's PHP. Use Every minute, as Laravel expects.",
  },
  {
    value: "php_script",
    label: "PHP script",
    hint: "Runs a .php file inside the project with the project's PHP.",
  },
  {
    value: "command",
    label: "Command",
    hint: "Runs a program (an .exe such as node or python) with arguments. No shell: pipes and && aren't available.",
  },
];

const field = "rounded border border-border-strong/70 bg-surface px-2 py-1.5 text-sm";

function formatTime(iso: string): string {
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

interface JobFormProps {
  job?: JobView;
  projects: ProjectView[];
  onSaved: (job: JobView) => void;
  onCancel: () => void;
}

/** Add or edit a job. Saving always goes through a confirmation of exactly what will run. */
export function JobForm({ job, projects, onSaved, onCancel }: JobFormProps) {
  const [projectId, setProjectId] = useState(job?.projectId ?? projects[0]?.id ?? "");
  const [kind, setKind] = useState<JobKind>(job?.kind ?? "artisan_schedule");
  const [name, setName] = useState(job?.name ?? "Laravel scheduler");
  const [script, setScript] = useState(job?.kind === "php_script" ? (job.args[0] ?? "") : "");
  const [program, setProgram] = useState(job?.program ?? "");
  const [argsText, setArgsText] = useState(
    job ? (job.kind === "php_script" ? job.args.slice(1) : job.args).join("\n") : "",
  );
  const [schedule, setSchedule] = useState<ScheduleChoice>(job ? fromCron(job.cron) : DEFAULT_CHOICE);
  const [timeoutMin, setTimeoutMin] = useState(job ? Math.round(job.timeoutSecs / 60) : 10);
  const [enabled, setEnabled] = useState(job?.enabled ?? true);
  const [error, setError] = useState<AppError | null>(null);
  const [checking, setChecking] = useState(false);
  const [review, setReview] = useState<{ request: JobRequest; preview: JobPreview } | null>(null);

  const extraArgs = argsText
    .split(/\r?\n/)
    .map((a) => a.trim())
    .filter(Boolean);

  const request = (): JobRequest => ({
    id: job?.id,
    projectId,
    name,
    kind,
    program: kind === "command" ? program : undefined,
    args: kind === "php_script" ? [script, ...extraArgs] : kind === "command" ? extraArgs : [],
    cron: toCron(schedule),
    timeoutSecs: Math.max(1, timeoutMin) * 60,
    enabled,
  });

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    setChecking(true);
    const req = request();
    try {
      setReview({ request: req, preview: await ipc.previewScheduledJob(req) });
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setChecking(false);
    }
  };

  const chooseKind = (k: JobKind) => {
    setKind(k);
    // Keep a name the user typed; replace the suggestion for the previous type.
    if (!job && KINDS.some((x) => x.label === name)) setName(KINDS.find((x) => x.value === k)?.label ?? name);
  };

  return (
    <form
      aria-label={job ? "Edit job" : "New job"}
      onSubmit={submit}
      className="flex max-w-2xl flex-col gap-5 text-sm"
    >
      <label className="flex flex-col gap-1">
        <span className="font-medium">Project</span>
        <select className={field} value={projectId} onChange={(e) => setProjectId(e.target.value)} required>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 font-medium">Type</legend>
        {KINDS.map((k) => (
          <div key={k.value} className="flex flex-col">
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="kind"
                value={k.value}
                checked={kind === k.value}
                onChange={() => chooseKind(k.value)}
                aria-describedby={`kind-hint-${k.value}`}
                className="accent-accent"
              />
              {k.label}
            </label>
            <span id={`kind-hint-${k.value}`} className="ml-6 text-xs text-fg-muted">
              {k.hint}
            </span>
          </div>
        ))}
      </fieldset>

      <label className="flex flex-col gap-1">
        <span className="font-medium">Name</span>
        <input
          className={field}
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={80}
          required
        />
      </label>

      {kind === "php_script" && (
        <Field label="Script" hint="Relative to the project folder." hintId="script-hint">
          <input
            className={`${field} font-mono`}
            value={script}
            onChange={(e) => setScript(e.target.value)}
            placeholder="scripts\sync.php"
            aria-describedby="script-hint"
            required
          />
        </Field>
      )}
      {kind === "command" && (
        <Field
          label="Program"
          hint="A program on the project's PATH (node, python, php…) or the full path to an .exe."
          hintId="program-hint"
        >
          <input
            className={`${field} font-mono`}
            value={program}
            onChange={(e) => setProgram(e.target.value)}
            placeholder="node"
            aria-describedby="program-hint"
            required
          />
        </Field>
      )}
      {kind !== "artisan_schedule" && (
        <Field label="Arguments" hint="One argument per line, passed exactly as written." hintId="args-hint">
          <textarea
            className={`${field} min-h-20 font-mono`}
            value={argsText}
            onChange={(e) => setArgsText(e.target.value)}
            placeholder={kind === "command" ? "scripts/sync.js\n--quiet" : "--force"}
            aria-describedby="args-hint"
          />
        </Field>
      )}

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 font-medium">Schedule</legend>
        <div className="flex flex-wrap items-center gap-2">
          <select
            aria-label="Schedule"
            className={field}
            value={schedule.preset}
            onChange={(e) => {
              const preset = e.target.value as ScheduleChoice["preset"];
              setSchedule((s) => ({ ...s, preset, cron: preset === "custom" ? toCron(s) : s.cron }));
            }}
          >
            {PRESETS.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
          </select>
          {schedule.preset === "weekly" && (
            <select
              aria-label="Day"
              className={field}
              value={schedule.weekday}
              onChange={(e) => setSchedule((s) => ({ ...s, weekday: Number(e.target.value) }))}
            >
              {WEEKDAYS.map((d, i) => (
                <option key={d} value={i}>
                  {d}
                </option>
              ))}
            </select>
          )}
          {(schedule.preset === "daily" || schedule.preset === "weekly") && (
            <input
              type="time"
              aria-label="Time"
              className={field}
              value={schedule.time}
              onChange={(e) => setSchedule((s) => ({ ...s, time: e.target.value }))}
              required
            />
          )}
          {schedule.preset === "custom" && (
            <input
              aria-label="Cron expression"
              className={`${field} font-mono`}
              value={schedule.cron}
              onChange={(e) => setSchedule((s) => ({ ...s, cron: e.target.value }))}
              placeholder="*/10 8-18 * * 1-5"
              required
            />
          )}
        </div>
        <span className="text-xs text-fg-muted">
          Local time. Jobs run while Devluma is open; runs missed while it is closed are not caught up.
          {schedule.preset === "custom" && " Five fields: minute hour day month weekday."}
        </span>
      </fieldset>

      <div className="flex flex-wrap gap-6">
        <label className="flex flex-col gap-1">
          <span className="font-medium">Time limit (minutes)</span>
          <input
            type="number"
            min={1}
            max={1440}
            className={`${field} w-28`}
            value={timeoutMin}
            onChange={(e) => setTimeoutMin(Number(e.target.value))}
          />
        </label>
        <label className="flex items-center gap-2 self-end pb-1.5">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
            className="accent-accent"
          />
          Run on schedule (untick to save it muted)
        </label>
      </div>

      {error && <ErrorPanel error={error} />}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" pending={checking}>
          Review
        </Button>
        <Button onClick={onCancel}>Cancel</Button>
      </div>

      {review && (
        <ConfirmChangeDialog
          open
          onOpenChange={(open) => !open && setReview(null)}
          title={job ? `Save changes to ${review.request.name}?` : `Schedule ${review.request.name}?`}
          target={[review.preview.command.executable, ...review.preview.command.args].join(" ")}
          impact={
            <div className="flex flex-col gap-2">
              <p>
                Runs in <span className="font-mono break-all">{review.preview.command.workingDir}</span>
                {review.preview.command.pathPrefix.length > 0 && (
                  <>
                    {" "}
                    with{" "}
                    <span className="font-mono break-all">
                      {review.preview.command.pathPrefix.join("; ")}
                    </span>{" "}
                    first on PATH (that run only)
                  </>
                )}
                .
              </p>
              <p>
                {review.preview.scheduleText}
                {review.request.enabled
                  ? ", next at "
                  : ". It is saved muted and won't run until you unmute it; it would run at "}
                {review.preview.nextRuns.map(formatTime).join(", ")}.
              </p>
              <p className="text-fg-muted">Stopped after {timeoutMin} minutes if it is still running.</p>
            </div>
          }
          confirmLabel={job ? "Save" : "Schedule job"}
          onConfirm={async () => {
            onSaved(await ipc.saveScheduledJob(review.request));
          }}
        />
      )}
    </form>
  );
}

/** A labelled control with its hint outside the label (so the hint isn't part of the name). */
function Field({
  label,
  hint,
  hintId,
  children,
}: {
  label: string;
  hint: string;
  hintId: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <label className="flex flex-col gap-1">
        <span className="font-medium">{label}</span>
        {children}
      </label>
      <span id={hintId} className="text-xs text-fg-muted">
        {hint}
      </span>
    </div>
  );
}
