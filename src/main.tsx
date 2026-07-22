import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { isTauri } from "@tauri-apps/api/core";
import { App } from "./App";
import "./styles.css";

if (isTauri() && navigator.userAgent.includes("Mac")) {
  document.documentElement.dataset.platform = "macos";
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
