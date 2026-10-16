import { useEffect, useRef, useState } from "react";
import { NavLink, Outlet, useLocation, useMatches } from "react-router";
import logoUrl from "../assets/logo.png";
import { CommandSlotContext } from "./commandBar";
import { NAV_ITEMS, type NavItem } from "./navigation";
import { StatusBar } from "./StatusBar";

/**
 * Desktop window layout under the native (theme-matched) title bar: command bar, navigation
 * pane, the page, and the status bar. Only the page area scrolls.
 */
export function AppShell() {
  const mainRef = useRef<HTMLElement>(null);
  const location = useLocation();
  const firstRender = useRef(true);
  const [commandSlot, setCommandSlot] = useState<HTMLElement | null>(null);

  // Move focus to the new page on navigation so screen-reader and keyboard users land on it.
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    mainRef.current?.focus();
  }, [location.pathname]);

  // Pane pages (map, list + detail, settings categories) fill the area; the rest flow padded.
  const paneLayout = useMatches().some((m) => (m.handle as RouteHandle | undefined)?.layout === "pane");
  // Settings is pinned to the bottom of the pane, Windows-style.
  const sections = NAV_ITEMS.filter((item) => item.path !== "/settings");
  const pinned = NAV_ITEMS.filter((item) => item.path === "/settings");

  return (
    <CommandSlotContext.Provider value={commandSlot}>
      <div className="flex h-screen flex-col overflow-hidden bg-surface">
        <a
          href="#main"
          onClick={(e) => {
            // Focus explicitly instead of relying on hash navigation (the router owns the URL).
            e.preventDefault();
            mainRef.current?.focus();
          }}
          className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded focus:bg-surface focus:px-3 focus:py-2"
        >
          Skip to content
        </a>
        <header className="flex h-11 shrink-0 items-center gap-3 border-b border-border bg-surface-sunken pr-3 pl-3.5">
          <span className="flex w-10 shrink-0 items-center gap-2.5 font-display text-sm font-semibold split:w-48">
            <img src={logoUrl} alt="" className="size-5" />
            <span className="sr-only split:not-sr-only">Devluma</span>
          </span>
          <div
            ref={setCommandSlot}
            aria-label="Page commands"
            role="group"
            className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5"
          />
        </header>

        <div className="flex min-h-0 flex-1">
          <nav
            aria-label="Main"
            className="flex w-14 shrink-0 flex-col gap-0.5 border-r border-border bg-surface-sunken p-2 split:w-52 split:overflow-y-auto"
          >
            {sections.map((item) => (
              <NavItemLink key={item.path} item={item} />
            ))}
            <div className="mt-auto border-t border-border pt-2">
              {pinned.map((item) => (
                <NavItemLink key={item.path} item={item} />
              ))}
            </div>
          </nav>

          <main
            id="main"
            ref={mainRef}
            tabIndex={-1}
            // `relative` contains absolutely positioned content (sr-only labels) so it scrolls and
            // clips with the page instead of stretching the window and adding a second scrollbar.
            className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto focus:outline-none"
          >
            {paneLayout ? (
              <Outlet />
            ) : (
              <div className="flex flex-col gap-5 px-7 py-6">
                <Outlet />
              </div>
            )}
          </main>
        </div>

        <StatusBar />
      </div>
    </CommandSlotContext.Provider>
  );
}

export interface RouteHandle {
  layout?: "pane";
}

function NavItemLink({ item: { path, label, icon: Icon } }: { item: NavItem }) {
  return (
    <NavLink
      to={path}
      end={path === "/"}
      className={({ isActive: active }) =>
        `group relative flex min-h-9 items-center justify-center gap-3 rounded-md px-2.5 text-sm transition-colors split:justify-start ${
          active
            ? "bg-surface-raised font-semibold text-fg before:absolute before:top-2 before:bottom-2 before:left-0 before:w-[3px] before:rounded-full before:bg-marker"
            : "text-fg-muted hover:bg-surface-raised hover:text-fg"
        }`
      }
    >
      <Icon aria-hidden className="size-4 shrink-0" />
      {/* Icon rail below the wide breakpoint keeps the accessible name. */}
      <span className="sr-only split:not-sr-only">{label}</span>
      {/* Rail flyout: the label on hover and keyboard focus (a native title never shows on focus). */}
      <span
        aria-hidden
        className="pointer-events-none absolute top-1/2 left-full z-20 ml-2 hidden -translate-y-1/2 rounded-md border border-border-strong bg-surface-raised px-2 py-1 text-xs font-medium whitespace-nowrap text-fg group-hover:block group-focus-visible:block split:group-hover:hidden split:group-focus-visible:hidden"
      >
        {label}
      </span>
    </NavLink>
  );
}
