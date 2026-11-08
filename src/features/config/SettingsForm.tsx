import { TriangleAlert } from "lucide-react";
import { useId, useMemo, useState } from "react";
import { Button } from "../../components/Button";
import type { ConfigApplied, ConfigTarget, ConfigView, SettingView } from "../../types/dto";
import { SaveConfigDialog } from "./SaveConfigDialog";
import {
  UNLIMITED,
  describeEffective,
  splitSize,
  summarize,
  timeZoneNames,
  validateDraft,
  type Draft,
} from "./settingValues";

const input = "rounded-md border border-border bg-surface px-2 py-1.5 font-mono tabular-nums";

interface SettingsFormProps {
  target: ConfigTarget;
  view: ConfigView;
  onApplied: (result: ConfigApplied) => void;
}

/** The friendly side: a few well-known settings per tool, written into the real config. */
export function SettingsForm({ target, view, onApplied }: SettingsFormProps) {
  const [draft, setDraft] = useState<Draft>({ values: {}, extensions: {} });
  const [saving, setSaving] = useState(false);
  const disabled = view.settingsUnavailable !== null;

  const groups = useMemo(() => {
    const map = new Map<string, SettingView[]>();
    for (const s of view.settings) map.set(s.group, [...(map.get(s.group) ?? []), s]);
    return [...map.entries()];
  }, [view.settings]);

  const current = (s: SettingView): string | null =>
    s.key in draft.values ? (draft.values[s.key] ?? null) : s.value;
  const setValue = (s: SettingView, value: string | null) =>
    setDraft((d) => {
      const values = { ...d.values };
      if (value === s.value) delete values[s.key];
      else values[s.key] = value;
      return { ...d, values };
    });
  const setExtension = (name: string, enabled: boolean, original: boolean) =>
    setDraft((d) => {
      const extensions = { ...d.extensions };
      if (enabled === original) delete extensions[name];
      else extensions[name] = enabled;
      return { ...d, extensions };
    });

  const errors = view.settings
    .map((s) => {
      const v = draft.values[s.key];
      return v == null ? null : validateDraft(s, v);
    })
    .filter((e): e is string => e !== null);
  const changeCount = Object.keys(draft.values).length + Object.keys(draft.extensions).length;
  const warning = uploadWarning(view.settings, current);

  if (view.settings.length === 0 && view.extensions === null) {
    return (
      <p className="text-sm text-fg-muted">This service has no simple settings. Use the advanced editor.</p>
    );
  }

  return (
    <form
      aria-label="Settings"
      className="flex flex-col gap-5 text-sm"
      onSubmit={(e) => {
        e.preventDefault();
        if (changeCount > 0 && errors.length === 0) setSaving(true);
      }}
    >
      {view.settingsUnavailable && (
        <p className="flex gap-2 rounded-md border border-warning/40 bg-warning-subtle p-3">
          <TriangleAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-warning" />
          {view.settingsUnavailable}
        </p>
      )}
      <p className="text-fg-muted">
        {view.settingsFile
          ? `Saved into ${view.files.find((f) => f.key === view.settingsFile)?.label ?? "the configuration file"}, the same file the advanced editor shows. Everything else in it stays as it is.`
          : "Devluma writes these into the configuration it generates before every start."}{" "}
        {view.applies === "reload"
          ? "A running server reloads after the check passes."
          : "A running service uses them after a restart."}
      </p>

      {groups.map(([group, settings]) => (
        <fieldset key={group} disabled={disabled} className="flex flex-col gap-3">
          <legend className="label-caps mb-1 text-fg-muted">{group}</legend>
          {settings.map((s) => (
            <SettingRow key={s.key} setting={s} value={current(s)} onChange={(v) => setValue(s, v)} />
          ))}
        </fieldset>
      ))}
      {warning && (
        <p role="status" className="flex gap-2 text-warning">
          <TriangleAlert aria-hidden className="mt-0.5 size-4 shrink-0" />
          {warning}
        </p>
      )}

      {view.extensions && (
        <Extensions
          extensions={view.extensions}
          draft={draft.extensions}
          disabled={disabled}
          onChange={setExtension}
        />
      )}

      <div className="sticky bottom-0 flex flex-wrap items-center gap-3 border-t border-border bg-surface py-3">
        <Button type="submit" variant="primary" disabled={disabled || changeCount === 0 || errors.length > 0}>
          Review and save
        </Button>
        <Button disabled={changeCount === 0} onClick={() => setDraft({ values: {}, extensions: {} })}>
          Discard changes
        </Button>
        <span className="text-fg-muted" aria-live="polite">
          {errors.length > 0
            ? errors[0]
            : changeCount === 0
              ? "No changes."
              : `${changeCount} ${changeCount === 1 ? "change" : "changes"}`}
        </span>
      </div>

      {saving && (
        <SaveConfigDialog
          target={target}
          change={{ kind: "settings", values: draft.values, extensions: draft.extensions }}
          title="Save these settings?"
          summary={summarize(view.settings, draft)}
          managed={view.settingsFile === null}
          onClose={() => setSaving(false)}
          onApplied={(result) => {
            setDraft({ values: {}, extensions: {} });
            onApplied(result);
          }}
        />
      )}
    </form>
  );
}

/** PHP rejects uploads bigger than the whole request; say so before the user saves. */
function uploadWarning(settings: SettingView[], current: (s: SettingView) => string | null): string | null {
  const upload = settings.find((s) => s.key === "upload_max_filesize");
  const post = settings.find((s) => s.key === "post_max_size");
  if (!upload || !post) return null;
  const u = current(upload) ?? upload.default;
  const p = current(post) ?? post.default;
  if (u === null || p === null || p === UNLIMITED || u === UNLIMITED) return null;
  return Number(p) < Number(u)
    ? "“Largest request” is smaller than “Largest upload”, so uploads that size will still fail."
    : null;
}

function SettingRow({
  setting: s,
  value,
  onChange,
}: {
  setting: SettingView;
  value: string | null;
  onChange: (value: string | null) => void;
}) {
  const id = useId();
  const helpId = `${id}-help`;
  const isDefault = value === null;
  return (
    <div className="grid grid-cols-1 gap-x-6 gap-y-1 border-b border-border pb-3 last:border-b-0 sm:grid-cols-[minmax(12rem,1fr)_minmax(16rem,1.2fr)]">
      <div className="flex flex-col gap-0.5">
        <label htmlFor={id} className="font-medium">
          {s.label}
        </label>
        <span id={helpId} className="text-xs text-pretty text-fg-muted">
          {s.help}
        </span>
      </div>
      <div className="flex flex-col gap-1">
        <Control id={id} helpId={helpId} setting={s} value={value} onChange={onChange} />
        <span className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
          {s.value === null && s.raw !== null && value === null ? (
            <span>
              Set in the file as <span className="font-mono">{s.raw}</span>, which Devluma can&apos;t show
              here. Saving replaces it.
            </span>
          ) : isDefault ? (
            <span>Default: {describeEffective({ ...s, value: null })}</span>
          ) : (
            <>
              <span>Default: {describeEffective({ ...s, value: null })}</span>
              <button type="button" className="text-accent hover:underline" onClick={() => onChange(null)}>
                Use default
              </button>
            </>
          )}
        </span>
      </div>
    </div>
  );
}

function Control({
  id,
  helpId,
  setting: s,
  value,
  onChange,
}: {
  id: string;
  helpId: string;
  setting: SettingView;
  value: string | null;
  onChange: (value: string) => void;
}) {
  const shown = value ?? s.default ?? "";
  const k = s.kind;
  switch (k.type) {
    case "toggle":
      return (
        <label className="flex items-center gap-2">
          <input
            id={id}
            type="checkbox"
            aria-describedby={helpId}
            className="size-4 accent-accent"
            checked={shown === "on"}
            onChange={(e) => onChange(e.target.checked ? "on" : "off")}
          />
          {shown === "on" ? "On" : "Off"}
        </label>
      );
    case "choice":
      return (
        <select
          id={id}
          aria-describedby={helpId}
          className="max-w-full rounded-md border border-border bg-surface px-2 py-1.5"
          value={k.options.some((o) => o.value === shown) ? shown : ""}
          onChange={(e) => onChange(e.target.value)}
        >
          {!k.options.some((o) => o.value === shown) && (
            <option value="" disabled>
              {s.raw ?? "Not set"}
            </option>
          )}
          {k.options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      );
    case "timezone":
      return <TimezoneInput id={id} helpId={helpId} value={shown} onChange={onChange} />;
    case "integer": {
      const special = k.special;
      const isSpecial = special !== null && shown === String(special.value);
      return (
        <div className="flex flex-wrap items-center gap-2">
          <input
            id={id}
            type="number"
            aria-describedby={helpId}
            min={k.min}
            max={k.max}
            step={1}
            disabled={isSpecial}
            className={`${input} w-32`}
            value={isSpecial ? "" : shown}
            onChange={(e) => onChange(e.target.value)}
          />
          <span className="text-fg-muted">{k.unit}</span>
          {special && (
            <label className="flex items-center gap-1.5">
              <input
                type="checkbox"
                className="size-4 accent-accent"
                checked={isSpecial}
                onChange={(e) =>
                  onChange(
                    e.target.checked
                      ? String(special.value)
                      : String(Math.max(k.min, Number(s.default) || k.min)),
                  )
                }
              />
              {special.label}
            </label>
          )}
        </div>
      );
    }
    case "size":
      return <SizeInput id={id} helpId={helpId} setting={s} kind={k} value={shown} onChange={onChange} />;
  }
}

function SizeInput({
  id,
  helpId,
  setting: s,
  kind,
  value,
  onChange,
}: {
  id: string;
  helpId: string;
  setting: SettingView;
  kind: Extract<SettingView["kind"], { type: "size" }>;
  value: string;
  onChange: (value: string) => void;
}) {
  const unlimited = value === UNLIMITED;
  // The unit is the user's choice from then on, so typing never makes it jump.
  const [unitChoice, setUnitChoice] = useState<"MB" | "GB">(() =>
    unlimited || value === "" ? "MB" : splitSize(Number(value))[1],
  );
  const factor = unitChoice === "GB" ? 1024 : 1;
  const displayed = unlimited || value === "" ? "" : String(Number(value) / factor);
  return (
    <div className="flex flex-wrap items-center gap-2">
      <input
        id={id}
        type="number"
        aria-describedby={helpId}
        min={unitChoice === "GB" ? 1 : kind.min}
        step={1}
        disabled={unlimited}
        className={`${input} w-28`}
        value={displayed}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange(e.target.value === "" ? "" : String(Math.round(n * factor)));
        }}
      />
      <select
        aria-label={`${s.label} unit`}
        disabled={unlimited}
        className="rounded-md border border-border bg-surface px-2 py-1.5"
        value={unitChoice}
        onChange={(e) => setUnitChoice(e.target.value as "MB" | "GB")}
      >
        <option value="MB">MB</option>
        <option value="GB">GB</option>
      </select>
      {kind.unlimited !== null && (
        <label className="flex items-center gap-1.5">
          <input
            type="checkbox"
            className="size-4 accent-accent"
            checked={unlimited}
            onChange={(e) => onChange(e.target.checked ? UNLIMITED : (s.default ?? String(kind.min)))}
          />
          No limit
        </label>
      )}
    </div>
  );
}

function TimezoneInput({
  id,
  helpId,
  value,
  onChange,
}: {
  id: string;
  helpId: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const listId = `${id}-zones`;
  const zones = useMemo(() => timeZoneNames(), []);
  return (
    <>
      <input
        id={id}
        type="text"
        aria-describedby={helpId}
        list={listId}
        spellCheck={false}
        autoComplete="off"
        className={`${input} w-full max-w-72`}
        value={value}
        onChange={(e) => onChange(e.target.value.trim())}
      />
      <datalist id={listId}>
        {zones.map((z) => (
          <option key={z} value={z} />
        ))}
      </datalist>
    </>
  );
}

function Extensions({
  extensions,
  draft,
  disabled,
  onChange,
}: {
  extensions: { name: string; enabled: boolean }[];
  draft: Record<string, boolean>;
  disabled: boolean;
  onChange: (name: string, enabled: boolean, original: boolean) => void;
}) {
  const [filter, setFilter] = useState("");
  const shown = extensions.filter((e) => e.name.includes(filter.trim().toLowerCase()));
  const enabledCount = extensions.filter((e) => draft[e.name] ?? e.enabled).length;
  return (
    <fieldset disabled={disabled} className="flex flex-col gap-2">
      <legend className="label-caps mb-1 text-fg-muted">Extensions</legend>
      <div className="flex flex-wrap items-center gap-3">
        <input
          type="search"
          aria-label="Filter extensions"
          placeholder="Filter"
          className="w-48 rounded-md border border-border bg-surface px-2 py-1.5"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <span className="text-xs text-fg-muted">
          {enabledCount} of {extensions.length} on. PHP checks that each one loads before the file is saved.
        </span>
      </div>
      <ul className="grid grid-cols-[repeat(auto-fill,minmax(10rem,1fr))] gap-x-4 gap-y-1">
        {shown.map((e) => {
          const on = draft[e.name] ?? e.enabled;
          return (
            <li key={e.name}>
              <label className="flex items-center gap-2 font-mono text-xs">
                <input
                  type="checkbox"
                  className="size-4 accent-accent"
                  checked={on}
                  onChange={(ev) => onChange(e.name, ev.target.checked, e.enabled)}
                />
                {e.name}
                {e.name in draft && <span className="sr-only"> (changed)</span>}
              </label>
            </li>
          );
        })}
      </ul>
      {shown.length === 0 && <p className="text-xs text-fg-muted">No extension matches “{filter}”.</p>}
    </fieldset>
  );
}
