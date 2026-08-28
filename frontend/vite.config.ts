import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/// <reference types="vitest" />

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    hmr: { protocol: "ws", host: "localhost", port: 1421 },
  },
  build: { target: "es2021", outDir: "dist", emptyOutDir: true },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
