// The only module that touches `@tauri-apps/api`. Everything else goes through
// `IpcAdapter`, so components and tests run without a live desktop runtime.
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AppError } from "./errors";

export type Unlisten = () => void;

export interface IpcAdapter {
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
  listen(event: string, handler: (payload: unknown) => void): Promise<Unlisten>;
}

export const tauriAdapter: IpcAdapter = {
  invoke: (command, args) => invoke(command, args),
  listen: (event, handler) => listen(event, (e) => handler(e.payload)),
};

/** Used when the UI runs in a plain browser (e.g. `pnpm dev`): every call fails honestly. */
export const unavailableAdapter: IpcAdapter = {
  invoke: () =>
    Promise.reject(
      new AppError({
        code: "DESKTOP_UNAVAILABLE",
        message: "The Devluma desktop backend is not available in this window.",
        recoverable: false,
      }),
    ),
  listen: () => Promise.resolve(() => {}),
};

let current: IpcAdapter = isTauri() ? tauriAdapter : unavailableAdapter;

export function getIpcAdapter(): IpcAdapter {
  return current;
}

/** Test hook: swap the adapter. Returns a function that restores the previous one. */
export function setIpcAdapter(adapter: IpcAdapter): () => void {
  const previous = current;
  current = adapter;
  return () => {
    current = previous;
  };
}
