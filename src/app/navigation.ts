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

export interface NavItem {
  path: string;
  label: string;
  icon: LucideIcon;
}

// Order and names from prd.md §8 / docs/design-brief.md §2.
export const NAV_ITEMS: NavItem[] = [
  { path: "/", label: "Overview", icon: LayoutDashboard },
  { path: "/projects", label: "Projects", icon: FolderGit2 },
  { path: "/services", label: "Services", icon: Server },
  { path: "/scheduler", label: "Scheduler", icon: CalendarClock },
  { path: "/runtimes", label: "Runtimes & Packages", icon: Package },
  { path: "/sites", label: "Sites & Domains", icon: Globe },
  { path: "/databases", label: "Databases", icon: Database },
  { path: "/ssl", label: "SSL Certificates", icon: ShieldCheck },
  { path: "/hosts", label: "Hosts File", icon: FileText },
  { path: "/logs", label: "Logs & Diagnostics", icon: ScrollText },
  { path: "/settings", label: "Settings", icon: Settings },
];
