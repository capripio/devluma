import type { PackageView } from "../../types/dto";

/** `services/install.rs` `OPERATION_TYPE`. */
export const INSTALL_OPERATION = "install_package";

export function installedVersions(p: PackageView): string[] {
  return p.versions.filter((v) => v.installation?.status === "installed").map((v) => v.version);
}
