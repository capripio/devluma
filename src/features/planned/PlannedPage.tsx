import type { LucideIcon } from "lucide-react";
import { Button } from "../../components/Button";
import { EmptyState } from "../../components/EmptyState";
import { PageHeader } from "../../components/PageHeader";

export interface PlannedPageProps {
  title: string;
  description: string;
  icon: LucideIcon;
  emptyTitle: string;
  emptyDescription: string;
  /** The primary action this page will offer, rendered disabled until implemented. */
  actionLabel: string;
}

/**
 * Honest placeholder for sections whose backend isn't built yet: the primary action is
 * visible but disabled with an explanation, and nothing claims an OS action happened.
 * Each feature replaces its route with a real page in its implementation phase.
 */
export function PlannedPage({
  title,
  description,
  icon,
  emptyTitle,
  emptyDescription,
  actionLabel,
}: PlannedPageProps) {
  const reasonId = `${title.replace(/\W+/g, "-").toLowerCase()}-unavailable`;
  return (
    <>
      <PageHeader
        title={title}
        description={description}
        actions={
          <Button variant="primary" disabled aria-describedby={reasonId}>
            {actionLabel}
          </Button>
        }
      />
      <EmptyState
        icon={icon}
        title={emptyTitle}
        description={
          <>
            <p>{emptyDescription}</p>
            <p id={reasonId} className="mt-2">
              <strong className="font-medium text-fg">Not available yet.</strong> This section isn't
              implemented in this build of Devluma.
            </p>
          </>
        }
      />
    </>
  );
}
