import type { ReactNode } from "react";
import { Commands } from "../app/commandBar";

interface PageHeaderProps {
  title: string;
  description?: ReactNode;
  /** Page commands; they go to the shell's command bar. */
  actions?: ReactNode;
}

export function PageHeader({ title, description, actions }: PageHeaderProps) {
  return (
    <header className="flex min-w-0 flex-col gap-1">
      <h1 className="font-display text-2xl font-semibold tracking-tight">{title}</h1>
      {description && <p className="max-w-prose text-sm text-pretty text-fg-muted">{description}</p>}
      {actions && <Commands>{actions}</Commands>}
    </header>
  );
}
