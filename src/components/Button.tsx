import { LoaderCircle } from "lucide-react";
import type { ButtonHTMLAttributes } from "react";

type Variant = "primary" | "secondary" | "danger" | "ghost";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg hover:bg-accent-hover enabled:active:brightness-90 border-transparent",
  secondary:
    "bg-surface-raised text-fg hover:bg-surface-sunken enabled:active:bg-border/60 border-border-strong/70",
  danger: "bg-danger text-white hover:bg-danger-hover enabled:active:brightness-90 border-transparent",
  ghost: "bg-transparent text-fg hover:bg-surface-raised enabled:active:bg-border/60 border-transparent",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  /** Shows a spinner and blocks further clicks (prevents duplicate submissions). */
  pending?: boolean;
}

export function Button({
  variant = "secondary",
  pending = false,
  disabled,
  className = "",
  children,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || pending}
      aria-busy={pending || undefined}
      className={`inline-flex min-h-8 items-center justify-center gap-2 rounded border px-3 py-1 text-sm font-medium whitespace-nowrap transition-[background-color,border-color,color,filter,translate] duration-150 motion-safe:enabled:active:translate-y-px disabled:cursor-not-allowed disabled:opacity-60 ${VARIANTS[variant]} ${className}`}
      {...rest}
    >
      {pending && <LoaderCircle aria-hidden className="size-4 animate-spin" />}
      {children}
    </button>
  );
}
