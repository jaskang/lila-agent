import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "electron-vite";

export default defineConfig({
  main: {},
  renderer: {
    resolve: {
      alias: {
        "@": resolve(import.meta.dirname, "src/renderer/src"),
      },
    },
    plugins: [react(), tailwindcss()],
  },
});
