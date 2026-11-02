/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and no screen clearing (its own logs share the terminal).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", target: "es2021" },
  // The theme-token test reads styles.css as text (`?raw`); vitest blanks CSS otherwise.
  test: { css: { include: [/styles\.css/] } },
});
