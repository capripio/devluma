import { useState, type ReactNode } from "react";
import { Link } from "react-router";
import { ipc } from "../../lib/ipc";
import { useAsync } from "../../lib/useAsync";
import type { NewSiteOptions, ServiceView } from "../../types/dto";
import { LocalCaNotice } from "../certificates/LocalCaNotice";
import { suggestDomain } from "../sites/SiteForm";

const input = "rounded-md border border-border bg-surface px-2 py-1.5";

/**
 * "Create a website at <domain>" for a new project (ADR 0020): domain (following the name
 * until edited), web server and HTTPS. The site is created right away, so the fields say what
 * it does. `site` is undefined when no site is wanted or there's no web server.
 */
export function useNewSite(
  name: string,
  services: ServiceView[],
): { site: NewSiteOptions | undefined; fields: (disabled: boolean) => ReactNode } {
  // The domain suffix from Settings; ".test" if preferences can't be read.
  const suffix = useAsync(() =>
    ipc.getPreferences().then(
      (p) => p.preferences.domainSuffix,
      () => "test",
    ),
  );
  const [domain, setDomain] = useState<string | null>(null);
  const [createSite, setCreateSite] = useState(true);
  const [webId, setWebId] = useState<string | null>(null);
  const [https, setHttps] = useState(false);

  const webServers = services.filter((s) => s.role === "web_server");
  const webValue = webServers.find((w) => w.id === webId) ?? webServers[0];
  const domainValue =
    domain ?? (name.trim() ? suggestDomain(name, suffix.status === "success" ? suffix.data : "test") : "");
  const site =
    createSite && webValue ? { domain: domainValue, webServerServiceId: webValue.id, https } : undefined;

  const fields = (disabled: boolean) => (
    <fieldset className="flex flex-col gap-2" disabled={disabled}>
      <legend className="sr-only">Website</legend>
      {webServers.length === 0 ? (
        <p className="text-fg-muted">
          No web server yet, so no site is created. Add Nginx, Apache or Caddy on the{" "}
          <Link to="/services" className="text-accent hover:underline">
            Services
          </Link>{" "}
          page, then add a site from the project.
        </p>
      ) : (
        <>
          <div className="flex flex-wrap items-end gap-4">
            <label className="flex items-center gap-2 py-1.5">
              <input type="checkbox" checked={createSite} onChange={(e) => setCreateSite(e.target.checked)} />
              Create a website at
            </label>
            <label className="flex flex-col gap-1">
              <span className="sr-only">Domain</span>
              <input
                required={createSite}
                disabled={!createSite}
                value={domainValue}
                onChange={(e) => setDomain(e.target.value)}
                placeholder="my-app.test"
                className={`${input} w-56 font-mono disabled:opacity-60`}
              />
            </label>
            {webServers.length > 1 ? (
              <label className="flex flex-col gap-1">
                <span className="text-fg-muted">Web server</span>
                <select
                  value={webValue?.id}
                  disabled={!createSite}
                  onChange={(e) => setWebId(e.target.value)}
                  className={input}
                >
                  {webServers.map((w) => (
                    <option key={w.id} value={w.id}>
                      {w.instanceName} — {w.packageName} (port {w.port})
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <span className="py-1.5 text-fg-muted">
                on {webValue?.instanceName} (port {webValue?.port})
              </span>
            )}
          </div>
          {createSite && (
            <>
              <label className="flex items-center gap-2">
                <input type="checkbox" checked={https} onChange={(e) => setHttps(e.target.checked)} />
                Also serve over HTTPS with a local development certificate
              </label>
              {https && <LocalCaNotice domain={domainValue} />}
              <p className="text-xs text-fg-muted">
                The site gets its web server config and a line in the Windows hosts file (Windows may ask for
                permission). Review or change it later on the project page.
              </p>
            </>
          )}
        </>
      )}
    </fieldset>
  );
  return { site, fields };
}
