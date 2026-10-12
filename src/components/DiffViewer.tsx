import { CopyButton } from "./CopyButton";

interface DiffViewerProps {
  /** Unified diff text. */
  diff: string;
  label: string;
}

/** Unified diff with additions/removals marked by symbol and colour (never colour alone). */
export function DiffViewer({ diff, label }: DiffViewerProps) {
  const lines = diff.split("\n").filter((l, i, all) => !(i === all.length - 1 && l === ""));
  if (lines.length === 0) {
    return <p className="text-sm text-fg-muted">No changes.</p>;
  }
  return (
    <figure className="flex flex-col gap-1">
      <figcaption className="flex items-center justify-between text-xs text-fg-muted">
        {label}
        <CopyButton value={diff} label={label.toLowerCase()} />
      </figcaption>
      <pre
        aria-label={label}
        className="max-h-72 overflow-auto rounded-md border border-border bg-surface-sunken p-2 font-mono text-xs"
      >
        {lines.map((line, i) => {
          const kind =
            line.startsWith("+") && !line.startsWith("+++")
              ? "add"
              : line.startsWith("-") && !line.startsWith("---")
                ? "remove"
                : "context";
          return (
            <div
              key={i}
              className={
                kind === "add"
                  ? "bg-success-subtle text-success"
                  : kind === "remove"
                    ? "bg-danger-subtle text-danger"
                    : "text-fg-muted"
              }
            >
              <span className="sr-only">
                {kind === "add" ? "Added: " : kind === "remove" ? "Removed: " : ""}
              </span>
              {line || " "}
            </div>
          );
        })}
      </pre>
    </figure>
  );
}
