import { useState, type ReactNode } from "react";
import { createBrowserRouter, RouterProvider } from "react-router";
import { SetupGate } from "../features/setup/SetupGate";
import { OperationsProvider } from "./operations";
import { routes } from "./routes";
import { StartupGate } from "./startup";

/** App-wide providers, shared with tests (which use a memory router instead). */
export function AppProviders({ children }: { children: ReactNode }) {
  return (
    <StartupGate>
      <OperationsProvider>
        <SetupGate>{children}</SetupGate>
      </OperationsProvider>
    </StartupGate>
  );
}

export function App() {
  const [router] = useState(() => createBrowserRouter(routes));
  return (
    <AppProviders>
      <RouterProvider router={router} />
    </AppProviders>
  );
}
