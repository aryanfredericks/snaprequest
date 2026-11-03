import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// `npm run dev` serves the UI on :5173 and forwards the API to a local `snapreq serve` (:7878).
const server = process.env.SNAPREQ_DEV_SERVER ?? "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [svelte()],
  build: {
    // Embedded into the server binary (rust-embed, ADR-021); hashed names under assets/.
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: "assets",
  },
  server: {
    proxy: {
      "/api": { target: server, ws: true },
      "/healthz": server,
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
  // Component tests mount Svelte's browser build.
  resolve: process.env.VITEST ? { conditions: ["browser"] } : undefined,
});
