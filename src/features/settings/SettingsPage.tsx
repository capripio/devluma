import {
  Cpu,
  Download,
  HardDrive,
  Info,
  type LucideIcon,
  Network,
  Palette,
  Power,
  RefreshCw,
  Share2,
  Shield,
  Wand2,
  Globe,
} from "lucide-react";
import { useRef, useState, type ReactNode } from "react";
import { Link, Navigate, useParams } from "react-router";
import { THEME_PREFERENCES, useThemePreference, type ThemePreference } from "../../app/theme";
import { Button } from "../../components/Button";
import { ErrorPanel } from "../../components/ErrorPanel";
import { LoadingState } from "../../components/LoadingState";
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

type PrefSection = "sites-terminal" | "ports" | "startup" | "downloads";

interface Category {
  slug: string;
  label: string;
  icon: LucideIcon;
  /** Forms backed by the stored preferences (loaded once for all of them). */
  prefs?: PrefSection;
  render?: () => ReactNode;
}

// docs/design-brief.md §5 Settings sections, one per pane. Diagnostics live on Logs & Diagnostics.
const CATEGORIES: Category[] = [
  { slug: "appearance", label: "Appearance", icon: Palette, render: () => <AppearanceSettings /> },
  { slug: "sites-terminal", label: "Sites & terminal", icon: Globe, prefs: "sites-terminal" },
  { slug: "ports", label: "Ports", icon: Network, prefs: "ports" },
  { slug: "startup", label: "Startup behavior", icon: Power, prefs: "startup" },
  { slug: "downloads", label: "Downloads", icon: Download, prefs: "downloads" },
  {
    slug: "runtime-defaults",
    label: "Runtime defaults",
    icon: Cpu,
    render: () => <RuntimeDefaultsSettings />,
  },
  { slug: "storage", label: "Storage", icon: HardDrive, render: () => <StorageSettings /> },
  { slug: "setup", label: "Setup", icon: Wand2, render: () => <SetupSettings /> },
  { slug: "sharing", label: "Sharing with ngrok", icon: Share2, render: () => <NgrokSettings /> },
  { slug: "security", label: "Security & privacy", icon: Shield, render: () => <SecuritySettings /> },
  { slug: "updates", label: "Updates", icon: RefreshCw, render: () => <UpdateSettings /> },
  { slug: "about", label: "About", icon: Info, render: () => <AboutSettings /> },
];

export function SettingsPage() {
  const { section } = useParams();
  const category = CATEGORIES.find((c) => c.slug === (section ?? "appearance"));
  if (!category) return <Navigate to="/settings" replace />;

  return (
    <div className="flex min-h-0 flex-1">
      <nav
        aria-label="Settings categories"
        className="flex w-14 shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border bg-surface-raised px-2 py-5 wide:w-60"
      >
        {CATEGORIES.map((c) => (
          <Link
            key={c.slug}
            to={c.slug === "appearance" ? "/settings" : `/settings/${c.slug}`}
            title={c.label}
            className={`relative flex min-h-9 items-center justify-center gap-3 rounded-md px-3 text-sm transition-colors wide:justify-start ${
              c.slug === category.slug
                ? "bg-surface font-semibold before:absolute before:top-2 before:bottom-2 before:left-0 before:w-[3px] before:rounded-full before:bg-marker"
                : "text-fg-muted hover:bg-surface-sunken hover:text-fg"
            }`}
            aria-current={c.slug === category.slug ? "page" : undefined}
          >
            <c.icon aria-hidden className="size-4 shrink-0" />
            <span className="sr-only wide:not-sr-only">{c.label}</span>
          </Link>
        ))}
      </nav>
      <div className="flex min-w-0 flex-1 flex-col gap-5 overflow-y-auto px-7 py-6">
        <PageHeader title="Settings" />
        <div className="flex max-w-3xl flex-col gap-5">
          {category.prefs ? <PreferencePane section={category.prefs} /> : category.render?.()}
        </div>
      </div>
    </div>
  );
}

function AppearanceSettings() {
  const [theme, setTheme] = useThemePreference();
  return (
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
      <p className="text-xs text-fg-muted">The window&apos;s title bar follows the same choice.</p>
    </SettingsSection>
  );
}

function SetupSettings() {
  return (
    <SettingsSection id="setup-heading" title="Setup">
      <p className="text-fg-muted">
        The setup wizard chooses Devluma&apos;s folder and downloads a starting set of tools. Running it again
        doesn&apos;t remove anything; already installed tools are skipped.
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
  );
}

function PreferencePane({ section }: { section: PrefSection }) {
  const prefs = useAsync(ipc.getPreferences);
  if (prefs.status === "pending") return <LoadingState label="Loading settings…" shape="panel" rows={5} />;
  if (prefs.status === "error") return <ErrorPanel error={prefs.error} onRetry={prefs.reload} />;
  return <PreferenceForms view={prefs.data} section={section} />;
}

function PreferenceForms({ view: loaded, section }: { view: PreferencesView; section: PrefSection }) {
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
  switch (section) {
    case "sites-terminal":
      return <SitesAndTerminalSettings view={view} save={save} />;
    case "ports":
      return <PortSettings view={view} save={save} />;
    case "startup":
      return <StartupSettings view={view} save={save} onChanged={reload} />;
    case "downloads":
      return <DownloadSettings view={view} save={save} />;
  }
}
