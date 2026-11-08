import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// Every page test gets the plain editor (see codeEditorMock.tsx).
vi.mock("../features/config/CodeEditor", () => import("./codeEditorMock"));

afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute("data-theme");
  try {
    localStorage.clear();
  } catch {
    // ignore
  }
});
