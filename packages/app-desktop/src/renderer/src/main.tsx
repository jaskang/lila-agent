import "./index.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { initTheme } from "./lib/theme.ts";

if (window.lila?.windowMaterial === "vibrant") {
  document.documentElement.dataset.windowMaterial = "vibrant";
}

initTheme();

const root = document.getElementById("root");
if (!root) throw new Error("#root not found");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
