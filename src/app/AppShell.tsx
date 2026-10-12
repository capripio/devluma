import { ListChecks } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { NavLink, Outlet, useLocation } from "react-router";
import { OperationTray } from "../components/OperationTray";
import { APP_ICON as AppIcon, NAV_ITEMS } from "./navigation";
import { isActive, needsRecovery, useOperations } from "./operations";

const TRAY_ID = "operation-tray";

export function AppShell() {
  const { operations } = useOperations();
  const activeCount = operations.filter(isActive).length;
  const recoveryCount = operations.filter(needsRecovery).length;
  const [trayOpen, setTrayOpen] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const location = useLocation();
  const firstRender = useRef(true);

  // Move focus to the new page on navigation so screen-reader and keyboard users land on it.
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    mainRef.current?.focus();
  }, [location.pathname]);

  return (
    <div className="flex h-screen flex-col">
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
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-border bg-surface-raised px-4">
        <div className="flex items-center gap-2 font-semibold">
          <AppIcon aria-hidden className="size-5 text-accent" />
          Devluma
        </div>
        <button
          type="button"
          aria-expanded={trayOpen}
          aria-controls={TRAY_ID}
          onClick={() => setTrayOpen((o) => !o)}
          className="inline-flex items-center gap-2 rounded-md px-2 py-1 text-sm hover:bg-surface-sunken"
        >
          <ListChecks aria-hidden className="size-4" />
          Operations
          {activeCount > 0 && (
            <span className="rounded-full bg-accent px-1.5 text-xs text-accent-fg">
              {activeCount}
              <span className="sr-only"> active</span>
            </span>
          )}
          {recoveryCount > 0 && (
            <span className="rounded-full bg-warning-subtle px-1.5 text-xs text-warning">
              {recoveryCount} need{recoveryCount === 1 ? "s" : ""} recovery
            </span>
          )}
        </button>
      </header>

      <div className="flex min-h-0 flex-1">
        <nav
          aria-label="Main"
          className="flex w-14 shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border bg-surface-raised p-2 wide:w-56"
        >
          {NAV_ITEMS.map(({ path, label, icon: Icon }) => (
            <NavLink
              key={path}
              to={path}
              end={path === "/"}
              title={label}
              className={({ isActive: active }) =>
                `flex items-center gap-3 rounded-md px-2 py-2 text-sm ${
                  active ? "bg-accent-subtle font-medium text-accent" : "text-fg hover:bg-surface-sunken"
                }`
              }
            >
              <Icon aria-hidden className="size-4 shrink-0" />
              {/* Icon rail below the wide breakpoint keeps the accessible name. */}
              <span className="sr-only wide:not-sr-only">{label}</span>
            </NavLink>
          ))}
        </nav>

        <div className="flex min-w-0 flex-1 flex-col">
          <main
            id="main"
            ref={mainRef}
            tabIndex={-1}
            className="min-h-0 flex-1 overflow-y-auto p-6 focus:outline-none"
          >
            <div className="mx-auto flex max-w-6xl flex-col gap-6">
              <Outlet />
            </div>
          </main>
          <OperationTray id={TRAY_ID} hidden={!trayOpen} />
        </div>
      </div>
    </div>
  );
}
