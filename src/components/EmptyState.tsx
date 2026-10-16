import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  description: ReactNode;
  /** The one recommended next action (app_flow.md §14). */
  action?: ReactNode;
}

export function EmptyState({ icon: Icon, title, description, action }: EmptyStateProps) {
  return (
    <section
      aria-label={title}
      className="flex flex-col items-center gap-3 rounded-md border border-dashed border-border px-6 py-10 text-center"
    >
      <Icon aria-hidden className="size-8 text-fg-muted" />
      <h2 className="text-base font-semibold">{title}</h2>
      <div className="max-w-prose text-sm text-pretty text-fg-muted">{description}</div>
      {action && <div className="mt-1">{action}</div>}
    </section>
  );
}
