import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "electron-vite";

export default defineConfig({
  main: {},
  // Sandboxed preload scripts must be CommonJS.
  preload: {
    build: {
      rolldownOptions: { output: { format: "cjs", entryFileNames: "[name].cjs" } },
    },
  },
  renderer: {
    resolve: {
      alias: {
        "@": resolve(import.meta.dirname, "src/renderer/src"),
      },
    },
    plugins: [react(), tailwindcss()],
    worker: { format: "es" },
  },
});
