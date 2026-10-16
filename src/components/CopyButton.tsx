import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";

interface CopyButtonProps {
  value: string;
  /** What is being copied, for the accessible name, e.g. "operation ID". */
  label: string;
}

export function CopyButton({ value, label }: CopyButtonProps) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (state === "idle") return;
    const t = setTimeout(() => setState("idle"), 2000);
    return () => clearTimeout(t);
  }, [state]);

  const copy = () => {
    navigator.clipboard.writeText(value).then(
      () => setState("copied"),
      () => setState("failed"),
    );
  };

  return (
    <button
      type="button"
      onClick={copy}
      aria-label={`Copy ${label}`}
      title={`Copy ${label}`}
      className="inline-flex items-center gap-1 rounded p-1 text-xs text-fg-muted transition-colors hover:bg-surface-sunken hover:text-fg active:bg-border/60"
    >
      {state === "copied" ? (
        <Check aria-hidden className="size-3.5" />
      ) : (
        <Copy aria-hidden className="size-3.5" />
      )}
      <span aria-live="polite">
        {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : ""}
      </span>
    </button>
  );
}
