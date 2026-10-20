import { defineConfig } from "@playwright/test";

const origin = "http://127.0.0.1:4173";

export default defineConfig({
  testDir: "tests",
  // The three engines share one machine, and each test is a measurement run.
  workers: 1,
  timeout: 5 * 60_000,
  reporter: "list",
  use: { baseURL: origin },
  webServer: {
    command: "npm run build && npm run preview -- --host 127.0.0.1 --port 4173 --strictPort",
    url: origin,
  },
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "firefox", use: { browserName: "firefox" } },
    // Playwright's WebKit build is not Safari. It has no iOS build either.
    { name: "webkit-not-safari", use: { browserName: "webkit" } },
  ],
});
