import {
  CalendarClock,
  Database,
  FileText,
  FolderGit2,
  Globe,
  LayoutDashboard,
  type LucideIcon,
  Package,
  ScrollText,
  Server,
  Settings,
  ShieldCheck,
} from "lucide-react";

/** Daily work first; tools you open now and then below it; Settings pinned to the foot. */
export type NavGroup = "work" | "tools" | "settings";

export interface NavItem {
  path: string;
  label: string;
  icon: LucideIcon;
  group: NavGroup;
}

export const NAV_GROUP_LABELS: Record<Exclude<NavGroup, "settings">, string> = {
  work: "Work",
  tools: "Tools",
};

// Names from prd.md §8 / docs/design-brief.md §2, grouped by how often they're used.
export const NAV_ITEMS: NavItem[] = [
  { path: "/", label: "Overview", icon: LayoutDashboard, group: "work" },
  { path: "/projects", label: "Projects", icon: FolderGit2, group: "work" },
  { path: "/sites", label: "Sites & Domains", icon: Globe, group: "work" },
  { path: "/services", label: "Services", icon: Server, group: "work" },
  { path: "/runtimes", label: "Runtimes & Packages", icon: Package, group: "tools" },
  { path: "/databases", label: "Databases", icon: Database, group: "tools" },
  { path: "/scheduler", label: "Scheduler", icon: CalendarClock, group: "tools" },
  { path: "/ssl", label: "SSL Certificates", icon: ShieldCheck, group: "tools" },
  { path: "/hosts", label: "Hosts File", icon: FileText, group: "tools" },
  { path: "/logs", label: "Logs & Diagnostics", icon: ScrollText, group: "tools" },
  { path: "/settings", label: "Settings", icon: Settings, group: "settings" },
];
