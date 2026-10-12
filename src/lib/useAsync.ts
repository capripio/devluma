import { useCallback, useEffect, useState } from "react";
import { toAppError, type AppError } from "./ipc/errors";

export type AsyncState<T> =
  { status: "pending" } | { status: "success"; data: T } | { status: "error"; error: AppError };

/**
 * Runs `load` on mount. `reload()` shows the pending state again (user-initiated retry);
 * `refresh()` refetches in the background and keeps current data on screen until the new
 * result arrives. Results from superseded runs or after unmount are ignored.
 */
export function useAsync<T>(
  load: () => Promise<T>,
): AsyncState<T> & { reload: () => void; refresh: () => void } {
  const [state, setState] = useState<AsyncState<T>>({ status: "pending" });
  const [run, setRun] = useState(0);

  useEffect(() => {
    let active = true;
    load().then(
      (data) => active && setState({ status: "success", data }),
      (error: unknown) => active && setState({ status: "error", error: toAppError(error) }),
    );
    return () => {
      active = false;
    };
    // `load` is expected to be stable (module-level function); re-run only on reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [run]);

  const reload = useCallback(() => {
    setState({ status: "pending" });
    setRun((n) => n + 1);
  }, []);
  const refresh = useCallback(() => setRun((n) => n + 1), []);

  return { ...state, reload, refresh };
}
