import { Package, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router";
import { useOnOperationsSettled } from "../../app/operations";
import { Button } from "../../components/Button";
import { EmptyState } from "../../components/EmptyState";
import { ErrorPanel } from "../../components/ErrorPanel";
import { PageHeader } from "../../components/PageHeader";
import { ipc } from "../../lib/ipc";
import { CATEGORY_LABELS } from "../../lib/format";
import { useAsync } from "../../lib/useAsync";
import { ARCHITECTURES, PACKAGE_CATEGORIES, type CatalogView, type PackageView } from "../../types/dto";
import { INSTALL_OPERATION, installedVersions } from "./packages";
import { LoadingState } from "../../components/LoadingState";

type InstallFilter = "all" | "installed" | "not_installed";

export function RuntimesPage() {
  const catalog = useAsync(ipc.listPackages);
  useOnOperationsSettled((op) => op.operationType === INSTALL_OPERATION, catalog.refresh);

  return (
    <>
      <PageHeader
        title="Runtimes & Packages"
        description="Install PHP, Node.js, Python, web servers and databases."
      />
      {catalog.status === "pending" && <LoadingState label="Loading catalog…" shape="table" columns={5} />}
      {catalog.status === "error" && <ErrorPanel error={catalog.error} onRetry={catalog.reload} />}
      {catalog.status === "success" && <Catalog view={catalog.data} />}
    </>
  );
}

function Catalog({ view }: { view: CatalogView }) {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<string>("all");
  const [arch, setArch] = useState<string>("all");
  const [installState, setInstallState] = useState<InstallFilter>("all");

  if (view.packages.length === 0) {
    return (
      <>
        <CatalogIssues view={view} />
        <EmptyState
          icon={Package}
          title="The package catalog is empty"
          description="This build doesn't include any reviewed packages yet. They arrive with updates."
        />
      </>
    );
  }

  const q = query.trim().toLowerCase();
  const matches = (p: PackageView) =>
    (q === "" || p.displayName.toLowerCase().includes(q) || p.id.includes(q)) &&
    (category === "all" || p.category === category) &&
    (arch === "all" || p.versions.some((v) => v.architecture === arch)) &&
    (installState === "all" || installedVersions(p).length > 0 === (installState === "installed"));
  const filtered = view.packages.filter(matches);
  const clear = () => {
    setQuery("");
    setCategory("all");
    setArch("all");
    setInstallState("all");
  };

  const selectClass = "rounded-md border border-border bg-surface px-2 py-1.5 text-sm";
  return (
    <>
      <CatalogIssues view={view} />
      <div role="search" className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-sm">
          <span className="text-fg-muted">Search</span>
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Name or id"
            className="w-56 rounded-md border border-border bg-surface px-2 py-1.5"
          />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span className="text-fg-muted">Category</span>
          <select value={category} onChange={(e) => setCategory(e.target.value)} className={selectClass}>
            <option value="all">All categories</option>
            {PACKAGE_CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {CATEGORY_LABELS[c]}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span className="text-fg-muted">Architecture</span>
          <select value={arch} onChange={(e) => setArch(e.target.value)} className={selectClass}>
            <option value="all">All</option>
            {ARCHITECTURES.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span className="text-fg-muted">Install state</span>
          <select
            value={installState}
            onChange={(e) => setInstallState(e.target.value as InstallFilter)}
            className={selectClass}
          >
            <option value="all">All</option>
            <option value="installed">Installed</option>
            <option value="not_installed">Not installed</option>
          </select>
        </label>
      </div>
      {(category === "all" || category === "cache") && (
        <p className="text-sm text-fg-muted">
          Redis and Memcached aren&apos;t offered: neither has an official Windows build. Run them in WSL or
          Docker and point your app at 127.0.0.1.
        </p>
      )}

      {filtered.length === 0 ? (
        <div className="flex flex-col items-start gap-2 text-sm text-fg-muted">
          <p>No packages match these filters.</p>
          <Button onClick={clear}>Clear filters</Button>
        </div>
      ) : (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Packages in catalog {view.catalogVersion}</caption>
            <thead className="bg-surface-raised text-fg-muted">
              <tr>
                <th scope="col" className="px-3 py-2 font-medium">
                  Package
                </th>
                <th scope="col" className="px-3 py-2 font-medium">
                  Category
                </th>
                <th scope="col" className="px-3 py-2 font-medium">
                  Available versions
                </th>
                <th scope="col" className="px-3 py-2 font-medium">
                  Installed
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {filtered.map((p) => {
                const installed = installedVersions(p);
                return (
                  <tr key={p.id}>
                    <td className="px-3 py-2">
                      <Link to={`/runtimes/${p.id}`} className="font-medium text-accent hover:underline">
                        {p.displayName}
                      </Link>
                      <span className="ml-2 font-mono text-xs text-fg-muted">{p.id}</span>
                    </td>
                    <td className="px-3 py-2">{CATEGORY_LABELS[p.category]}</td>
                    <td className="px-3 py-2">{p.versions.length}</td>
                    <td className="px-3 py-2">{installed.length > 0 ? installed.join(", ") : "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function CatalogIssues({ view }: { view: CatalogView }) {
  if (view.issues.length === 0) return null;
  return (
    <section
      aria-label="Catalog problems"
      className="flex gap-3 rounded-md border border-warning/40 bg-warning-subtle p-4 text-sm"
    >
      <TriangleAlert aria-hidden className="mt-0.5 size-5 shrink-0 text-warning" />
      <div>
        <p className="font-medium text-fg">
          {view.issues.length} catalog {view.issues.length === 1 ? "entry was" : "entries were"} rejected and
          can't be installed.
        </p>
        <ul className="mt-1 list-inside list-disc text-fg">
          {view.issues.map((i, n) => (
            <li key={n}>
              {[i.packageId, i.version].filter(Boolean).join(" ")}
              {i.packageId ? ": " : ""}
              {i.message}
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
