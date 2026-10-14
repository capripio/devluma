import { useRef, useState } from "react";
import { THEME_PREFERENCES, useThemePreference, type ThemePreference } from "../../app/theme";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import type { Preferences, PreferencesView } from "../../types/dto";
import { NgrokSettings } from "../tunnels/NgrokSettings";
import { AboutSettings, RuntimeDefaultsSettings, SecuritySettings, StorageSettings } from "./InfoSections";
import {
  DownloadSettings,
  PortSettings,
  SettingsSection,
  SitesAndTerminalSettings,
  StartupSettings,
} from "./PreferenceSections";
import { UpdateSettings } from "./UpdateSettings";

const THEME_LABELS: Record<ThemePreference, string> = {
  system: "Match Windows",
  light: "Light",
  dark: "Dark",
};

// design.md §5 Settings sections. Diagnostics live on the Logs & Diagnostics page.
export function SettingsPage() {
  const [theme, setTheme] = useThemePreference();
  const prefs = useAsync(ipc.getPreferences);

  return (
    <>
      <PageHeader title="Settings" />

      <SettingsSection id="appearance-heading" title="Appearance">
        <fieldset>
          <legend className="text-fg-muted">Theme</legend>
          <div className="mt-2 flex flex-wrap gap-4">
            {THEME_PREFERENCES.map((pref) => (
              <label key={pref} className="flex items-center gap-2">
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
      </SettingsSection>

      {prefs.status === "pending" && (
        <p role="status" className="text-sm text-fg-muted">
          Loading settings…
        </p>
      )}
      {prefs.status === "error" && <ErrorPanel error={prefs.error} onRetry={prefs.reload} />}
      {prefs.status === "success" && <PreferenceForms view={prefs.data} />}

      <RuntimeDefaultsSettings />

      <StorageSettings />

      <SettingsSection id="setup-heading" title="Setup">
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
      </SettingsSection>

      <NgrokSettings />

      <SecuritySettings />

      <UpdateSettings />

      <AboutSettings />
    </>
  );
}

function PreferenceForms({ view: loaded }: { view: PreferencesView }) {
  // Each form sends its own fields on top of the latest stored preferences (the backend
  // validates the whole set and returns what it stored), so saves from different forms
  // never undo each other.
  const latest = useRef(loaded);
  const [view, setView] = useState(loaded);
  const update = (next: PreferencesView) => {
    latest.current = next;
    setView(next);
  };
  const save = async (patch: Partial<Preferences>) => {
    update(await ipc.setPreferences({ ...latest.current.preferences, ...patch }));
  };
  const reload = () => {
    ipc.getPreferences().then(update, () => undefined);
  };
  return (
    <>
      <SitesAndTerminalSettings view={view} save={save} />
      <PortSettings view={view} save={save} />
      <StartupSettings view={view} save={save} onChanged={reload} />
      <DownloadSettings view={view} save={save} />
    </>
  );
}
