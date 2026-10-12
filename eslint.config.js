import js from "@eslint/js";
import jsxA11y from "eslint-plugin-jsx-a11y";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "src-tauri"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  reactHooks.configs.flat.recommended,
  jsxA11y.flatConfigs.recommended,
  {
    languageOptions: { globals: globals.browser },
    rules: {
      // UI code must go through src/lib/ipc, never call Tauri APIs directly (trd.md §3).
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["@tauri-apps/*"],
              message: "Use the typed wrappers in src/lib/ipc instead of calling Tauri APIs directly.",
            },
          ],
        },
      ],
    },
  },
  {
    files: ["src/lib/ipc/adapter.ts"],
    rules: { "no-restricted-imports": "off" },
  },
);
