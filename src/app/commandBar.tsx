import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";

/**
 * The shell's command bar is one element; pages put their commands into it through
 * `<Commands>` so the actions sit where a desktop app keeps them, under the title bar.
 */
export const CommandSlotContext = createContext<HTMLElement | null>(null);

export function Commands({ children }: { children: ReactNode }) {
  const slot = useContext(CommandSlotContext);
  // Outside the shell (isolated component tests, recovery screen) render in place.
  if (!slot) return <div className="flex flex-wrap items-center gap-2">{children}</div>;
  return createPortal(children, slot);
}
