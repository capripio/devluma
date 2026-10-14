import type { TerminalProgram } from "../../types/dto";

export const TERMINAL_LABELS: Record<TerminalProgram, string> = {
  power_shell: "Windows PowerShell",
  command_prompt: "Command Prompt",
  cmder: "Cmder",
};

/** What an "Open terminal" action did, for a status line next to the button. */
export function terminalOpenedMessage(r: { program: TerminalProgram; pathPrefix: string[] }): string {
  const name = TERMINAL_LABELS[r.program];
  return r.pathPrefix.length > 0
    ? `Opened ${name} with ${r.pathPrefix.join(", ")} first on PATH (that window only).`
    : `Opened ${name}. No runtime is selected yet.`;
}
