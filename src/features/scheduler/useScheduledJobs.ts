import { useEffect } from "react";
import { events, ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";

/**
 * Scheduled jobs, kept current by `scheduler://job` events (a run started or ended, a job
 * changed) and a once-a-minute refresh so "next run" times move on.
 */
export function useScheduledJobs() {
  const jobs = useAsync(ipc.listScheduledJobs);
  const { refresh } = jobs;
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    events
      .onScheduledJob(() => refresh())
      .then((fn) => (disposed ? fn() : (unlisten = fn)))
      .catch(() => {});
    const timer = window.setInterval(refresh, 60_000);
    return () => {
      disposed = true;
      unlisten?.();
      window.clearInterval(timer);
    };
  }, [refresh]);
  return jobs;
}
