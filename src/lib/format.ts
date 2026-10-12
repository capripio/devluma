import type { PackageCategory } from "../types/dto";

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export const CATEGORY_LABELS: Record<PackageCategory, string> = {
  runtime: "Runtime",
  web_server: "Web server",
  database: "Database",
  cache: "Cache",
  mail: "Mail",
  tunnel: "Tunnel",
  tool: "Tool",
};
