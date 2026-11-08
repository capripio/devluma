import type { SettingView } from "../../types/dto";

// Canonical values match `domain/config_settings.rs`: sizes in whole MB or "unlimited",
// integers as decimal strings, toggles "on"/"off", choices by value, time zones by name.
export const UNLIMITED = "unlimited";

/** "512 MB", "2 GB", "No limit", "30 seconds", "On" — mirrors `config_settings::describe`. */
export function describeValue(setting: SettingView, value: string): string {
  const k = setting.kind;
  switch (k.type) {
    case "size": {
      if (value === UNLIMITED) return "No limit";
      const mb = Number(value);
      return mb >= 1024 && mb % 1024 === 0 ? `${mb / 1024} GB` : `${value} MB`;
    }
    case "integer":
      return k.special && String(k.special.value) === value ? k.special.label : `${value} ${k.unit}`;
    case "toggle":
      return value === "on" ? "On" : "Off";
    case "choice":
      return k.options.find((o) => o.value === value)?.label ?? value;
    case "timezone":
      return value;
  }
}

/** What applies now: the set value, else the default. */
export function effectiveValue(setting: SettingView): string | null {
  return setting.value ?? setting.default;
}

export function describeEffective(setting: SettingView): string {
  const v = effectiveValue(setting);
  if (v === null) return setting.defaultLabel ?? "Default";
  return describeValue(setting, v);
}

/** Why a draft value is invalid, or null when it's fine. Mirrors the backend's limits. */
export function validateDraft(setting: SettingView, value: string): string | null {
  const k = setting.kind;
  switch (k.type) {
    case "size": {
      if (value === UNLIMITED) return k.unlimited !== null ? null : "No unlimited option.";
      const n = Number(value);
      return Number.isInteger(n) && n >= k.min && n <= k.max
        ? null
        : `Enter ${k.min}–${k.max.toLocaleString()} MB.`;
    }
    case "integer": {
      const n = Number(value);
      if (value.trim() === "" || !Number.isInteger(n)) return "Enter a whole number.";
      if (k.special && n === k.special.value) return null;
      return n >= k.min && n <= k.max ? null : `Enter ${k.min}–${k.max.toLocaleString()}.`;
    }
    case "timezone":
      return /^[A-Za-z0-9_+-]+(\/[A-Za-z0-9_+-]+)*$/.test(value) && value.length <= 64
        ? null
        : "Use a time zone name such as Europe/London or UTC.";
    default:
      return null;
  }
}

/** A size in MB shown in the larger whole unit: 2048 → [2, "GB"]. */
export function splitSize(mb: number): [number, "MB" | "GB"] {
  return mb >= 1024 && mb % 1024 === 0 ? [mb / 1024, "GB"] : [mb, "MB"];
}

export interface Draft {
  /** key → new canonical value, or null to go back to the default. */
  values: Record<string, string | null>;
  extensions: Record<string, boolean>;
}

/** "Memory limit: 128 MB → 512 MB" lines for the confirmation dialog. */
export function summarize(settings: SettingView[], draft: Draft): string[] {
  const lines: string[] = [];
  for (const [key, next] of Object.entries(draft.values)) {
    const s = settings.find((x) => x.key === key);
    if (!s) continue;
    const before = s.value === null && s.raw !== null ? s.raw : describeEffective(s);
    const after =
      next === null ? `${describeEffective({ ...s, value: null })} (default)` : describeValue(s, next);
    lines.push(`${s.label}: ${before} → ${after}`);
  }
  for (const [name, on] of Object.entries(draft.extensions)) {
    lines.push(`Extension ${name}: ${on ? "turn on" : "turn off"}`);
  }
  return lines;
}

/** Time zone names the WebView knows, for suggestions (the backend and PHP still check). */
export function timeZoneNames(): string[] {
  try {
    const intl = Intl as unknown as { supportedValuesOf?: (key: string) => string[] };
    return ["UTC", ...(intl.supportedValuesOf?.("timeZone") ?? [])];
  } catch {
    return ["UTC"];
  }
}
