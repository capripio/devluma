import { ShieldCheck } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Button } from "../../components/Button";
import { ConfirmChangeDialog } from "../../components/ConfirmChangeDialog";
import { CopyButton } from "../../components/CopyButton";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { toAppError, type AppError } from "../../lib/ipc/errors";
import { useAsync } from "../../lib/useAsync";
import type { CertificateView } from "../../types/dto";
import { LoadingState } from "../../components/LoadingState";

type Pending = { kind: "trust" | "untrust" | "delete" | "renew"; cert: CertificateView };

export function CertificatesPage() {
  const certs = useAsync(ipc.listCertificates);
  return (
    <>
      <PageHeader
        title="SSL Certificates"
        description="Local development certificates. They are never publicly trusted."
      />
      {certs.status === "pending" && <LoadingState label="Loading certificates…" shape="table" columns={5} />}
      {certs.status === "error" && <ErrorPanel error={certs.error} onRetry={certs.reload} />}
      {certs.status === "success" && <Certificates certs={certs.data} onChanged={certs.refresh} />}
    </>
  );
}

function expiry(c: CertificateView): string {
  const date = new Date(c.notAfter).toLocaleDateString();
  if (c.expired) return `Expired ${date}`;
  return `${date} (${c.daysLeft} days)`;
}

function Certificates({ certs, onChanged }: { certs: CertificateView[]; onChanged: () => void }) {
  const ca = certs.find((c) => c.issuerType === "local_ca");
  const sites = certs.filter((c) => c.issuerType === "local_ca_signed");
  const [pending, setPending] = useState<Pending | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const createCa = () => {
    setCreating(true);
    setError(null);
    ipc.createLocalCa().then(
      () => {
        setCreating(false);
        onChanged();
      },
      (e: unknown) => {
        setCreating(false);
        setError(toAppError(e));
      },
    );
  };

  return (
    <>
      {error && <ErrorPanel error={error} />}
      <section
        aria-labelledby="ca-heading"
        className="flex flex-col gap-3 rounded-md border border-border p-4 text-sm"
      >
        <h2 id="ca-heading" className="font-semibold">
          Local certificate authority
        </h2>
        {!ca ? (
          <>
            <p className="text-fg-muted">
              Devluma signs site certificates with its own local CA. It can only sign local domains (.test,
              .localhost, .internal, .example, .invalid, .local), so it can&apos;t be used to impersonate real
              websites.
            </p>
            <div>
              <Button variant="primary" pending={creating} onClick={createCa}>
                Create local CA
              </Button>
            </div>
          </>
        ) : (
          <>
            <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1">
              <dt className="text-fg-muted">Name</dt>
              <dd>{ca.commonName}</dd>
              <dt className="text-fg-muted">Trusted on this computer</dt>
              <dd>
                {ca.trustedNow
                  ? "Yes, for your Windows account"
                  : "No — browsers will warn about Devluma's certificates"}
              </dd>
              <dt className="text-fg-muted">Valid until</dt>
              <dd>{expiry(ca)}</dd>
              <dt className="text-fg-muted">Private key</dt>
              <dd>{ca.keyProtection}</dd>
              <dt className="text-fg-muted">SHA-256</dt>
              <dd className="flex items-center gap-1 font-mono text-xs break-all">
                {ca.fingerprintSha256}
                <CopyButton value={ca.fingerprintSha256} label="CA fingerprint" />
              </dd>
            </dl>
            <div className="flex flex-wrap gap-2">
              {ca.trustedNow ? (
                <Button onClick={() => setPending({ kind: "untrust", cert: ca })}>Remove trust</Button>
              ) : (
                <Button variant="primary" onClick={() => setPending({ kind: "trust", cert: ca })}>
                  Trust on this computer
                </Button>
              )}
              <Button
                onClick={() => setPending({ kind: "delete", cert: ca })}
                disabled={ca.trustedNow || sites.length > 0}
                aria-describedby="ca-delete-hint"
              >
                Delete CA
              </Button>
            </div>
            {(ca.trustedNow || sites.length > 0) && (
              <p id="ca-delete-hint" className="text-xs text-fg-muted">
                To delete the CA, remove its trust and delete the site certificates it issued first.
              </p>
            )}
          </>
        )}
      </section>

      <section aria-labelledby="site-certs-heading" className="flex flex-col gap-2">
        <h2 id="site-certs-heading" className="text-sm font-semibold">
          Site certificates
        </h2>
        <GenerateForm onCreated={onChanged} />
        {sites.length === 0 ? (
          <EmptyState
            icon={ShieldCheck}
            title="No site certificates"
            description="Turn on HTTPS for a site, or generate a certificate above. Generating never trusts anything."
          />
        ) : (
          <div className="overflow-x-auto rounded-md border border-border">
            <table className="w-full text-left text-sm">
              <thead className="bg-surface-raised text-fg-muted">
                <tr>
                  {["Domains", "Expires", "Used by", "Key", "Actions"].map((h) => (
                    <th key={h} scope="col" className="px-3 py-2 font-medium">
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {sites.map((c) => (
                  <tr key={c.id} className="align-top">
                    <td className="px-3 py-2 font-mono">{c.sans.join(", ")}</td>
                    <td className="px-3 py-2">
                      {expiry(c)}
                      {c.expiringSoon && !c.expired && (
                        <div className="text-xs text-warning">Expires soon — renew</div>
                      )}
                      {c.expired && <div className="text-xs text-danger">Expired — renew</div>}
                    </td>
                    <td className="px-3 py-2">{c.boundSites.length > 0 ? c.boundSites.join(", ") : "—"}</td>
                    <td className="px-3 py-2 text-xs">
                      <div>{c.keyProtection}</div>
                      <div className="flex items-center gap-1 font-mono break-all text-fg-muted">
                        {c.privateKeyPath}
                        <CopyButton value={c.privateKeyPath} label={`key path for ${c.commonName}`} />
                      </div>
                    </td>
                    <td className="px-3 py-2">
                      <div className="flex flex-wrap gap-2">
                        <Button
                          onClick={() => setPending({ kind: "renew", cert: c })}
                          aria-label={`Renew ${c.commonName}`}
                        >
                          Renew
                        </Button>
                        <Button
                          onClick={() => setPending({ kind: "delete", cert: c })}
                          disabled={c.boundSites.length > 0}
                          aria-label={`Delete ${c.commonName}`}
                        >
                          Delete
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {pending && <PendingDialog pending={pending} onClose={() => setPending(null)} onDone={onChanged} />}
    </>
  );
}

function GenerateForm({ onCreated }: { onCreated: () => void }) {
  const [domains, setDomains] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.createDevCertificate(
        domains
          .split(/[\s,]+/)
          .map((d) => d.trim())
          .filter(Boolean),
      );
      setDomains("");
      onCreated();
    } catch (err) {
      setError(toAppError(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} aria-label="Generate certificate" className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-end gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-fg-muted">Domains (comma-separated)</span>
          <input
            required
            value={domains}
            onChange={(e) => setDomains(e.target.value)}
            placeholder="my-app.test, api.my-app.test"
            className="w-80 rounded-md border border-border bg-surface px-2 py-1.5 font-mono"
          />
        </label>
        <Button type="submit" pending={busy}>
          Generate certificate
        </Button>
      </div>
      {error && <ErrorPanel error={error} />}
    </form>
  );
}

const DIALOGS = {
  trust: {
    title: "Trust Devluma's local CA on this computer?",
    confirm: "Trust",
    impact: (
      <ul className="list-inside list-disc space-y-1">
        <li>Adds the CA to your Windows account&apos;s trusted root certificates (CurrentUser\Root).</li>
        <li>Windows will show its own security warning; confirm it to finish.</li>
        <li>
          Browsers that use Windows&apos; store (Edge, Chrome) will then accept Devluma&apos;s site
          certificates. Firefox uses its own store unless its &quot;enterprise roots&quot; setting is on.
        </li>
        <li>The CA can only sign local domains, so it can&apos;t vouch for real websites.</li>
      </ul>
    ),
    rollback: "Remove trust at any time from this page. No administrator permission is needed.",
  },
  untrust: {
    title: "Remove trust for Devluma's local CA?",
    confirm: "Remove trust",
    impact:
      "Removes the CA from your trusted root certificates (Windows will ask you to confirm). Browsers will warn about Devluma's certificates again. Keys and certificate files are kept.",
    rollback: "You can trust it again later.",
  },
  renew: {
    title: "Renew this certificate?",
    confirm: "Renew",
    impact:
      "Issues a new certificate and key for the same domains, replacing the files in place. Running web servers using it are reloaded.",
    rollback: undefined,
  },
  delete: {
    title: "Delete this certificate and its private key?",
    confirm: "Delete",
    impact: "The certificate and its key are deleted permanently.",
    rollback: undefined,
  },
} as const;

function PendingDialog({
  pending,
  onClose,
  onDone,
}: {
  pending: Pending;
  onClose: () => void;
  onDone: () => void;
}) {
  const d = DIALOGS[pending.kind];
  return (
    <ConfirmChangeDialog
      open
      destructive={pending.kind === "delete"}
      onOpenChange={(o) => !o && onClose()}
      title={d.title}
      target={pending.kind === "delete" ? pending.cert.privateKeyPath : pending.cert.commonName}
      impact={d.impact}
      rollback={d.rollback}
      confirmLabel={d.confirm}
      onConfirm={async () => {
        const id = pending.cert.id;
        if (pending.kind === "trust") await ipc.trustLocalCa();
        else if (pending.kind === "untrust") await ipc.removeLocalCaTrust();
        else if (pending.kind === "renew") await ipc.renewCertificate(id);
        else await ipc.deleteCertificate(id);
        onDone();
      }}
    />
  );
}
