import { ShieldAlert } from "lucide-react";
import type { ReactNode } from "react";

/** Explains an administrator prompt before Windows shows it (app_flow.md §14). */
export function PermissionExplainer({ children }: { children: ReactNode }) {
  return (
    <div className="flex gap-2 rounded-md border border-warning/40 bg-warning-subtle p-3 text-sm text-fg">
      <ShieldAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-warning" />
      <div>
        <p className="font-medium">Windows will ask for administrator approval</p>
        <div className="mt-1">{children}</div>
      </div>
    </div>
  );
}
