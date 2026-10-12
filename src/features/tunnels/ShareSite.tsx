import { useState } from "react";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { ipc } from "../../lib/ipc";
import { toAppError } from "../../lib/ipc/errors";
import type { SiteView, TunnelView } from "../../types/dto";

/**
 * Share/Stop sharing for one site. Sharing makes the site reachable from the internet, so it
 * always asks first and says exactly what that means.
 */
export function ShareSite({
  site,
  tunnel,
  onChanged,
}: {
  site: SiteView;
  tunnel: TunnelView | undefined;
  onChanged: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const stop = () => {
    setStopping(true);
    setError(null);
    ipc.stopTunnel(site.id).then(
      () => {
        setStopping(false);
        onChanged();
      },
      (e: unknown) => {
        setStopping(false);
        setError(toAppError(e).message);
      },
    );
  };

  return (
    <div className="flex flex-col gap-1">
      {tunnel?.running ? (
        <Button pending={stopping} onClick={stop} aria-label={`Stop sharing ${site.domain}`}>
          Stop sharing
        </Button>
      ) : (
        <Button onClick={() => setConfirming(true)} aria-label={`Share ${site.domain} with ngrok`}>
          Share
        </Button>
      )}
      {error && <span className="text-xs text-danger">{error}</span>}
      {confirming && (
        <ConfirmChangeDialog
          open
          onOpenChange={(o) => !o && setConfirming(false)}
          title={`Share ${site.domain} on the internet?`}
          target={site.url}
          impact={
            <ul className="list-inside list-disc space-y-1">
              <li>ngrok gives this site a public https address. Anyone who has the address can open it.</li>
              <li>Requests go through ngrok&apos;s servers, using your ngrok account.</li>
              <li>Sharing stops when you click Stop sharing or close Devluma.</li>
            </ul>
          }
          rollback="Stop sharing at any time; the public address stops working immediately."
          confirmLabel="Share"
          onConfirm={async () => {
            await ipc.startTunnel(site.id);
            onChanged();
          }}
        />
      )}
    </div>
  );
}

/** The public address of a shared site, shown under its local URL. */
export function SharedAddress({ tunnel }: { tunnel: TunnelView | undefined }) {
  if (!tunnel) return null;
  if (!tunnel.running) {
    return <p className="text-xs text-danger">Sharing stopped: {tunnel.lastError ?? "ngrok exited."}</p>;
  }
  return tunnel.publicUrl ? (
    <div className="flex items-center gap-1 text-xs">
      <span className="rounded bg-warning/15 px-1 font-medium text-warning">Public</span>
      <span className="font-mono break-all">{tunnel.publicUrl}</span>
      <CopyButton value={tunnel.publicUrl} label={`public address of ${tunnel.domain}`} />
    </div>
  ) : null;
}
