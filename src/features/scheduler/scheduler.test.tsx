import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { axeViolations, createMockAdapter, defaultHandlers, renderApp } from "../../test/utils";
import type { JobView } from "../../types/dto";
import { fromCron, toCron } from "./schedule";

const project = {
  id: "p1",
  name: "shop",
  rootPath: String.raw`C:\Projects\shop`,
  projectType: "laravel",
  localDomain: null,
  notes: null,
  isFavorite: false,
  createdAt: "2026-10-01T00:00:00Z",
  updatedAt: "2026-10-01T00:00:00Z",
  lastOpenedAt: null,
  detection: { kind: "laravel", label: "Laravel", suggestedDocumentRoot: "public", virtualEnvScripts: null },
  runtimes: [],
};

const command = {
  executable: String.raw`C:\Devluma\bin\php\8.3.14\php.exe`,
  args: ["artisan", "schedule:run"],
  workingDir: String.raw`C:\Projects\shop`,
  pathPrefix: [String.raw`C:\Devluma\bin\php\8.3.14`],
};

function job(over: Partial<JobView> = {}): JobView {
  return {
    id: "j1",
    projectId: "p1",
    projectName: "shop",
    name: "Laravel scheduler",
    kind: "artisan_schedule",
    program: null,
    args: [],
    cron: "* * * * *",
    scheduleText: "Every minute",
    enabled: true,
    timeoutSecs: 600,
    status: "active",
    nextRun: "2026-10-16T09:01:00+05:00",
    lastRun: null,
    runningSince: null,
    command,
    problem: null,
    ...over,
  };
}

describe("schedule presets", () => {
  it("turns presets into cron expressions and back", () => {
    const base = { time: "02:30", weekday: 1, cron: "" };
    expect(toCron({ ...base, preset: "minute" })).toBe("* * * * *");
    expect(toCron({ ...base, preset: "15" })).toBe("*/15 * * * *");
    expect(toCron({ ...base, preset: "daily" })).toBe("30 2 * * *");
    expect(toCron({ ...base, preset: "weekly" })).toBe("30 2 * * 1");
    expect(fromCron("*/5 * * * *").preset).toBe("5");
    expect(fromCron("30 2 * * *")).toMatchObject({ preset: "daily", time: "02:30" });
    expect(fromCron("0 9 * * 0")).toMatchObject({ preset: "weekly", time: "09:00", weekday: 0 });
    expect(fromCron("*/10 8-18 * * 1-5")).toMatchObject({ preset: "custom", cron: "*/10 8-18 * * 1-5" });
  });
});

describe("scheduler page", () => {
  it("lists jobs with their status in words, never colour alone", async () => {
    const jobs = [
      job(),
      job({ id: "j2", name: "Sync", status: "executing", runningSince: "2026-10-16T09:00:00+05:00" }),
      job({ id: "j3", name: "Report", status: "muted", enabled: false, nextRun: null }),
      job({ id: "j4", name: "Prune", status: "failed" }),
    ];
    const { container } = await renderApp(
      "/scheduler",
      createMockAdapter({ ...defaultHandlers, list_scheduled_jobs: () => jobs }),
    );
    const table = await screen.findByRole("table", { name: "Scheduled jobs" });
    for (const label of ["Active", "Executing", "Muted", "Failed"]) {
      expect(within(table).getByText(label)).toBeInTheDocument();
    }
    expect(screen.getByText("1 job failed")).toBeInTheDocument();
    expect(await axeViolations(container)).toEqual([]);
  });

  it("shows exactly what will run before scheduling a job", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project],
      preview_scheduled_job: () => ({
        command,
        cron: "* * * * *",
        scheduleText: "Every minute",
        nextRuns: ["2026-10-16T09:01:00+05:00", "2026-10-16T09:02:00+05:00", "2026-10-16T09:03:00+05:00"],
      }),
      save_scheduled_job: () => job(),
    });
    await renderApp("/scheduler/new", mock);
    const form = await screen.findByRole("form", { name: "New job" });
    expect(within(form).getByRole("radio", { name: "Laravel scheduler" })).toBeChecked();
    await userEvent.click(within(form).getByRole("button", { name: "Review" }));

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("php.exe artisan schedule:run");
    expect(dialog).toHaveTextContent(String.raw`C:\Projects\shop`);
    expect(dialog).toHaveTextContent("Every minute, next at");
    expect(mock.calls.some((c) => c.command === "save_scheduled_job")).toBe(false);

    await userEvent.click(within(dialog).getByRole("button", { name: "Schedule job" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "save_scheduled_job")?.args).toEqual({
        request: {
          id: undefined,
          projectId: "p1",
          name: "Laravel scheduler",
          kind: "artisan_schedule",
          program: undefined,
          args: [],
          cron: "* * * * *",
          timeoutSecs: 600,
          enabled: true,
        },
      }),
    );
  });

  it("sends commands as a program plus one argument per line, never a shell string", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_projects: () => [project],
      preview_scheduled_job: () => {
        throw {
          code: "VALIDATION_ERROR",
          message: "Couldn't find nodee.exe on this project's PATH.",
          recoverable: true,
        };
      },
    });
    await renderApp("/scheduler/new", mock);
    const form = await screen.findByRole("form", { name: "New job" });
    await userEvent.click(within(form).getByRole("radio", { name: "Command" }));
    await userEvent.type(within(form).getByLabelText("Program"), "nodee");
    await userEvent.type(within(form).getByLabelText("Arguments"), "scripts/sync.js{enter}--quiet");
    await userEvent.selectOptions(within(form).getByLabelText("Schedule"), "15");
    await userEvent.click(within(form).getByRole("button", { name: "Review" }));

    expect(await within(form).findByRole("alert")).toHaveTextContent("Couldn't find nodee.exe");
    expect(mock.calls.find((c) => c.command === "preview_scheduled_job")?.args).toMatchObject({
      request: {
        kind: "command",
        program: "nodee",
        args: ["scripts/sync.js", "--quiet"],
        cron: "*/15 * * * *",
      },
    });
  });

  it("mutes a job and runs it on demand", async () => {
    const mock = createMockAdapter({
      ...defaultHandlers,
      list_scheduled_jobs: () => [job()],
      list_scheduled_job_runs: () => [],
      set_scheduled_job_enabled: () => job({ enabled: false, status: "muted" }),
      run_scheduled_job_now: () => ({
        id: "r1",
        jobId: "j1",
        trigger: "manual",
        status: "running",
        exitCode: null,
        message: null,
        startedAt: "2026-10-16T09:00:00Z",
        finishedAt: null,
      }),
    });
    await renderApp("/scheduler/j1", mock);
    expect(await screen.findByRole("heading", { level: 1, name: "Laravel scheduler" })).toBeInTheDocument();
    expect(screen.getByText(/artisan schedule:run/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Mute" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "set_scheduled_job_enabled")?.args).toEqual({
        jobId: "j1",
        enabled: false,
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Run now" }));
    await waitFor(() =>
      expect(mock.calls.find((c) => c.command === "run_scheduled_job_now")?.args).toEqual({ jobId: "j1" }),
    );
  });
});
