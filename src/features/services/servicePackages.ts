/**
 * What the add-service form needs to know per package: default ports and what to tell the
 * user before creating it. The backend validates everything again (and owns the real rules).
 */
export interface ServicePackageInfo {
  defaultName: string;
  defaultPort: number;
  /** Second port for the tool's own UI (backend: `ServiceBehavior::admin_port`). */
  adminPort?: { label: string; default: number };
  /** Shown under the data-folder fields for databases. */
  dataNote?: string;
  /** Shown for every service of this package. */
  note?: string;
}

const PACKAGES: Record<string, ServicePackageInfo> = {
  nginx: { defaultName: "web", defaultPort: 8080 },
  apache: { defaultName: "apache", defaultPort: 8081 },
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
    adminPort: { label: "Web interface port", default: 8025 },
    note: "Mailpit catches every email your apps send to its SMTP port and shows it in its web interface. Nothing is delivered to real recipients.",
  },
};

export function servicePackageInfo(packageId: string): ServicePackageInfo {
  return PACKAGES[packageId] ?? { defaultName: "service", defaultPort: 8080 };
}
