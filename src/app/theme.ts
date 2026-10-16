import { useCallback, useState } from "react";
import { setNativeWindowTheme } from "../lib/ipc";

export const THEME_PREFERENCES = ["system", "light", "dark"] as const;
export type ThemePreference = (typeof THEME_PREFERENCES)[number];

const STORAGE_KEY = "devluma.theme";

// Theme is a per-window UI convenience, so localStorage is appropriate here. App settings
// that the backend cares about go to SQLite (Phase 2).
export function readThemePreference(): ThemePreference {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    return THEME_PREFERENCES.includes(value as ThemePreference) ? (value as ThemePreference) : "system";
  } catch {
    return "system";
  }
}

export function applyThemePreference(pref: ThemePreference): void {
  const root = document.documentElement;
  if (pref === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", pref);
  setNativeWindowTheme(pref === "system" ? null : pref);
}

export function useThemePreference(): [ThemePreference, (pref: ThemePreference) => void] {
  const [pref, setPref] = useState<ThemePreference>(readThemePreference);
  const update = useCallback((next: ThemePreference) => {
    setPref(next);
    applyThemePreference(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Storage unavailable: the choice still applies for this session.
    }
  }, []);
  return [pref, update];
}
