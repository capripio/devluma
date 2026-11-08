import { describe, expect, it } from "vitest";

import type { DefaultPorts } from "../../types/dto";
import { servicePackageInfo } from "./servicePackages";

const ports: DefaultPorts = {
  nginx: 8080,
  apache: 8081,
  caddy: 8082,
  caddyAdmin: 2020,
  mariadb: 3306,
  postgresql: 5432,
  mongodb: 27017,
  mailpit: 1026,
  mailpitWeb: 8026,
};

describe("servicePackageInfo", () => {
  it("takes each package's ports from Settings, including its own second port", () => {
    expect(servicePackageInfo("caddy", ports)).toMatchObject({
      defaultPort: 8082,
      adminPort: { label: "Admin API port", default: 2020 },
    });
    expect(servicePackageInfo("mailpit", ports)).toMatchObject({
      defaultPort: 1026,
      adminPort: { default: 8026 },
    });
    expect(servicePackageInfo("nginx", ports).adminPort).toBeUndefined();
  });

  it("falls back to the built-in defaults without Settings", () => {
    expect(servicePackageInfo("caddy")).toMatchObject({ defaultPort: 80, adminPort: { default: 2019 } });
  });
});
