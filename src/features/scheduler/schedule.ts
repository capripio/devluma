// Schedule presets for the job form. Everything is stored as a 5-field cron expression; the
// backend (`domain::schedule`) validates it and is the source of truth for run times.

export type Preset = "minute" | "5" | "15" | "30" | "hourly" | "daily" | "weekly" | "custom";

export interface ScheduleChoice {
  preset: Preset;
  /** "HH:MM" for daily/weekly. */
  time: string;
  /** 0 = Sunday … 6 = Saturday, for weekly. */
  weekday: number;
  /** For custom. */
  cron: string;
}

export const PRESETS: { value: Preset; label: string }[] = [
  { value: "minute", label: "Every minute" },
  { value: "5", label: "Every 5 minutes" },
  { value: "15", label: "Every 15 minutes" },
  { value: "30", label: "Every 30 minutes" },
  { value: "hourly", label: "Every hour" },
  { value: "daily", label: "Every day at…" },
  { value: "weekly", label: "Every week on…" },
  { value: "custom", label: "Custom (cron expression)" },
];

export const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

export const DEFAULT_CHOICE: ScheduleChoice = {
  preset: "minute",
  time: "02:00",
  weekday: 1,
  cron: "* * * * *",
};

function hm(time: string): [number, number] {
  const [h = "0", m = "0"] = time.split(":");
  return [Number(h), Number(m)];
}

export function toCron(c: ScheduleChoice): string {
  switch (c.preset) {
    case "minute":
      return "* * * * *";
    case "5":
    case "15":
    case "30":
      return `*/${c.preset} * * * *`;
    case "hourly":
      return "0 * * * *";
    case "daily": {
      const [h, m] = hm(c.time);
      return `${m} ${h} * * *`;
    }
    case "weekly": {
      const [h, m] = hm(c.time);
      return `${m} ${h} * * ${c.weekday}`;
    }
    case "custom":
      return c.cron.trim();
  }
}

/** The form state for a stored expression: a preset when it is one, otherwise custom. */
export function fromCron(cron: string): ScheduleChoice {
  const f = cron.trim().split(/\s+/);
  const base = { ...DEFAULT_CHOICE, cron: f.join(" ") };
  const n = (s: string | undefined) => (s !== undefined && /^\d+$/.test(s) ? Number(s) : null);
  const pad = (v: number) => String(v).padStart(2, "0");
  if (f.length !== 5) return { ...base, preset: "custom" };
  const [m, h, dom, mon, dow] = f as [string, string, string, string, string];
  const rest = (d: string) => dom === "*" && mon === "*" && dow === d;
  if (m === "*" && h === "*" && rest("*")) return { ...base, preset: "minute" };
  if (["*/5", "*/15", "*/30"].includes(m) && h === "*" && rest("*"))
    return { ...base, preset: m.slice(2) as Preset };
  if (m === "0" && h === "*" && rest("*")) return { ...base, preset: "hourly" };
  const mm = n(m);
  const hh = n(h);
  if (mm !== null && hh !== null && mm < 60 && hh < 24 && dom === "*" && mon === "*") {
    const time = `${pad(hh)}:${pad(mm)}`;
    if (dow === "*") return { ...base, preset: "daily", time };
    const d = n(dow);
    if (d !== null && d <= 7) return { ...base, preset: "weekly", time, weekday: d % 7 };
  }
  return { ...base, preset: "custom" };
}
