import type { DefaultPorts } from "../../types/dto";

/**
 * What the add-service form needs to know per package: default ports and what to tell the
 * user before creating it. The backend validates everything again (and owns the real rules).
 */
export interface ServicePackageInfo {
  defaultName: string;
  defaultPort: number;
  /** Second port (backend: `ServiceBehavior::admin_port`). */
  adminPort?: AdminPortInfo;
  /** Shown under the data-folder fields for databases. */
  dataNote?: string;
  /** Shown for every service of this package. */
  note?: string;
}

export interface AdminPortInfo {
  label: string;
  default: number;
  /** Short tag next to the port in lists, e.g. "UI 8025". */
  short: string;
  /** A page to open in the browser (Mailpit), not an API (Caddy's admin endpoint). */
  webInterface: boolean;
}

const PACKAGES: Record<string, ServicePackageInfo> = {
  nginx: { defaultName: "web", defaultPort: 80 },
  apache: { defaultName: "apache", defaultPort: 80 },
  caddy: {
    defaultName: "caddy",
    defaultPort: 80,
    adminPort: { label: "Admin API port", default: 2019, short: "admin", webInterface: false },
    note: "Devluma reloads and stops Caddy through its admin API, which only listens on 127.0.0.1. Caddy's automatic HTTPS is turned off: HTTPS sites use certificates from Devluma's local CA.",
  },
  frankenphp: {
    defaultName: "frankenphp",
    defaultPort: 80,
    adminPort: { label: "Admin API port", default: 2019, short: "admin", webInterface: false },
    note: "Your own FrankenPHP (chosen in Settings → FrankenPHP). Sites on it use its built-in PHP; projects' PHP pins don't apply. Its php.ini is in the service folder.",
  },
  mariadb: {
    defaultName: "db",
    defaultPort: 3306,
    dataNote:
      "A new data folder is created with MariaDB's own tool. The root user has no password; the server only accepts connections from this computer (127.0.0.1).",
  },
  postgresql: {
    defaultName: "pg",
    defaultPort: 5432,
    dataNote:
      "A new data folder is created with PostgreSQL's initdb. The postgres user can sign in without a password from this computer only (127.0.0.1).",
  },
  mongodb: {
    defaultName: "mongo",
    defaultPort: 27017,
    dataNote:
      "MongoDB creates its files in the data folder on first start. Access control is off; the server only accepts connections from this computer (127.0.0.1).",
  },
  mailpit: {
    defaultName: "mail",
    defaultPort: 1025,
    adminPort: { label: "Web interface port", default: 8025, short: "UI", webInterface: true },
    note: "Mailpit catches every email your apps send to its SMTP port and shows it in its web interface. Nothing is delivered to real recipients.",
  },
};

/** Package ids whose default ports are set in Settings (keys of `DefaultPorts`). */
const PORT_SETTING: Partial<Record<string, keyof DefaultPorts>> = {
  nginx: "nginx",
  apache: "apache",
  caddy: "caddy",
  mariadb: "mariadb",
  postgresql: "postgresql",
  mongodb: "mongodb",
  mailpit: "mailpit",
};

/** The Settings key for each package's second port. */
const ADMIN_PORT_SETTING: Partial<Record<string, keyof DefaultPorts>> = {
  caddy: "caddyAdmin",
  mailpit: "mailpitWeb",
};

/** Info for `packageId`, with the default ports chosen in Settings when given. */
export function servicePackageInfo(packageId: string, ports?: DefaultPorts): ServicePackageInfo {
  const info = PACKAGES[packageId] ?? { defaultName: "service", defaultPort: 80 };
  const key = PORT_SETTING[packageId];
  if (!ports || !key) return info;
  const adminKey = ADMIN_PORT_SETTING[packageId];
  return {
    ...info,
    defaultPort: ports[key],
    adminPort: info.adminPort && adminKey ? { ...info.adminPort, default: ports[adminKey] } : info.adminPort,
  };
}

/** How to describe a service's second port; generic when the package isn't known here. */
export function adminPortInfo(packageId: string): AdminPortInfo {
  return (
    PACKAGES[packageId]?.adminPort ?? { label: "Second port", default: 0, short: "port", webInterface: false }
  );
}
