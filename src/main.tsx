import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { disableContextMenu } from "./app/contextMenu";
import { applyThemePreference, readThemePreference } from "./app/theme";
import "./styles/index.css";

applyThemePreference(readThemePreference());
disableContextMenu();

const root = document.getElementById("root");
if (!root) throw new Error("Root element #root not found");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
