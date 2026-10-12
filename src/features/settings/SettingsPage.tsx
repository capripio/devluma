import { THEME_PREFERENCES, useThemePreference, type ThemePreference } from "../../app/theme";
import { Button } from "../../components/Button";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { NgrokSettings } from "../tunnels/NgrokSettings";
import { UpdateSettings } from "./UpdateSettings";

const THEME_LABELS: Record<ThemePreference, string> = {
  system: "Match Windows",
  light: "Light",
  dark: "Dark",
};

// design.md §5 Settings sections. Appearance, Sharing and Updates work in this build; diagnostics
// live on the Logs & Diagnostics page.
const UNAVAILABLE_SECTIONS = [
  "Storage",
  "Downloads",
  "Runtime Defaults",
  "Startup Behavior",
  "Security & Privacy",
];

export function SettingsPage() {
  const [theme, setTheme] = useThemePreference();

  return (
    <>
      <PageHeader title="Settings" />

      <section aria-labelledby="appearance-heading" className="rounded-lg border border-border p-4">
        <h2 id="appearance-heading" className="text-sm font-semibold">
          Appearance
        </h2>
        <fieldset className="mt-3">
          <legend className="text-sm text-fg-muted">Theme</legend>
          <div className="mt-2 flex flex-wrap gap-4">
            {THEME_PREFERENCES.map((pref) => (
              <label key={pref} className="flex items-center gap-2 text-sm">
                <input
                  type="radio"
                  name="theme"
                  value={pref}
                  checked={theme === pref}
                  onChange={() => setTheme(pref)}
                  className="accent-accent"
                />
                {THEME_LABELS[pref]}
              </label>
            ))}
          </div>
        </fieldset>
      </section>

      <section
        aria-labelledby="setup-heading"
        className="flex flex-col gap-2 rounded-lg border border-border p-4 text-sm"
      >
        <h2 id="setup-heading" className="font-semibold">
          Setup
        </h2>
        <p className="text-fg-muted">
          The setup wizard chooses Devluma&apos;s folder and downloads a starting set of tools. Running it
          again doesn&apos;t remove anything; already installed tools are skipped.
        </p>
        <div>
          <Button
            onClick={() => {
              void ipc.reopenSetup().then(() => window.location.reload());
            }}
          >
            Run setup again
          </Button>
        </div>
      </section>

      <NgrokSettings />

      <UpdateSettings />

      <section
        aria-labelledby="unavailable-settings-heading"
        className="rounded-lg border border-dashed border-border p-4"
      >
        <h2 id="unavailable-settings-heading" className="text-sm font-semibold">
          Not available yet
        </h2>
        <p className="mt-1 text-sm text-fg-muted">These settings arrive with the features they control:</p>
        <ul className="mt-2 list-inside list-disc text-sm text-fg-muted">
          {UNAVAILABLE_SECTIONS.map((s) => (
            <li key={s}>{s}</li>
          ))}
        </ul>
      </section>
    </>
  );
}
