import { ShieldCheck, TriangleAlert } from "lucide-react";
import { Link } from "react-router";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";

/**
 * Shown when a site is asked to use HTTPS: whether this computer trusts Devluma's local CA
 * (checked live in the Windows certificate store), and if not, the way to the page that fixes
 * it. Applying a site may create the CA but never trusts it (ADR 0010).
 */
export function LocalCaNotice({ domain }: { domain: string }) {
  const certs = useAsync(ipc.listCertificates);
  if (certs.status !== "success") return null;
  const ca = certs.data.find((c) => c.issuerType === "local_ca");
  const url = `https://${domain.trim() || "your-site.test"}`;

  if (ca?.trustedNow) {
    return (
      <p className="flex items-center gap-1.5 text-xs text-fg-muted">
        <ShieldCheck aria-hidden className="size-4 text-success" />
        This computer trusts Devluma&apos;s local certificate authority, so browsers accept {url}.
      </p>
    );
  }
  return (
    <div
      role="status"
      className="flex flex-col gap-1.5 rounded-md border border-warning/40 bg-warning-subtle p-3 text-sm"
    >
      <p className="flex items-center gap-1.5 font-semibold text-warning">
        <TriangleAlert aria-hidden className="size-4" />
        SSL isn&apos;t set up on this computer yet
      </p>
      <p>
        {ca
          ? "Devluma's local certificate authority exists, but this computer doesn't trust it."
          : "Devluma's local certificate authority hasn't been created and trusted yet."}{" "}
        The site still works, but browsers will warn that {url} is not secure until you trust it.
      </p>
      <Link to="/ssl" className="self-start font-medium text-accent hover:underline">
        Set up SSL Certificates
      </Link>
    </div>
  );
}
