interface LoadingStateProps {
  /** Announced to screen readers and shown in the inline variant, e.g. "Loading projects…". */
  label: string;
  /** Outline of what is about to load, so the page doesn't jump when it arrives. */
  shape?: "table" | "panel" | "inline";
  /** Table columns to draw (table shape only). */
  columns?: number;
  rows?: number;
}

// Static by design: no shimmer or pulse, nothing to suppress under reduced motion.
function Bar({ className = "" }: { className?: string }) {
  return <span className={`block h-3 rounded-sm bg-surface-sunken ${className}`} />;
}

const WIDTHS = ["w-3/4", "w-1/2", "w-2/3", "w-1/3", "w-5/6"];

export function LoadingState({ label, shape = "table", columns = 4, rows = 3 }: LoadingStateProps) {
  if (shape === "inline") {
    return (
      <p role="status" className="text-sm text-fg-muted">
        {label}
      </p>
    );
  }

  return (
    <div role="status" aria-busy="true">
      <span className="sr-only">{label}</span>
      {shape === "table" ? (
        <div aria-hidden className="overflow-hidden rounded-md border border-border">
          <div className="flex gap-6 bg-surface-raised px-3 py-3">
            {Array.from({ length: columns }, (_, c) => (
              <Bar key={c} className="flex-1 bg-border/60" />
            ))}
          </div>
          {Array.from({ length: rows }, (_, r) => (
            <div key={r} className="flex gap-6 border-t border-border px-3 py-3.5">
              {Array.from({ length: columns }, (_, c) => (
                <span key={c} className="flex-1">
                  <Bar className={WIDTHS[(r + c) % WIDTHS.length]} />
                </span>
              ))}
            </div>
          ))}
        </div>
      ) : (
        <div aria-hidden className="flex flex-col gap-3 rounded-md border border-border p-4">
          <Bar className="w-1/4" />
          {Array.from({ length: rows }, (_, r) => (
            <Bar key={r} className={WIDTHS[r % WIDTHS.length]} />
          ))}
        </div>
      )}
    </div>
  );
}
