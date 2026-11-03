import { defineConfig, devices } from "@playwright/test";

// Run through scripts/web-e2e.sh, which starts the server and the mock API.
export default defineConfig({
  testDir: "e2e",
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  // On CI, failures also become annotations on the job (readable without the logs).
  reporter: process.env.CI
    ? [["github"], ["list"], ["html", { open: "never", outputFolder: "../target/web-e2e/report" }]]
    : "list",
  outputDir: "../target/web-e2e/results",
  use: {
    baseURL: process.env.SNAPREQ_E2E_URL ?? "http://127.0.0.1:7881",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
