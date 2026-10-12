import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute("data-theme");
  try {
    localStorage.clear();
  } catch {
    // ignore
  }
});
