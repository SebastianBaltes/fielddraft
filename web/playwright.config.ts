import { defineConfig, devices } from "@playwright/test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

/**
 * Playwright config for FieldDraft smoke and viewport tests.
 *
 * We only run Chromium with WebGPU enabled — WebGPU is a hard requirement,
 * no fallback. Firefox/Safari will land once their WebGPU support is stable
 * in CI-grade headless mode.
 *
 * Per-instance: base URL (port + scheme) is derived exactly like vite.config.ts
 * so `fielddraft` (5173) and `fielddraft-a` (5174) test their *own* dev server
 * instead of whichever one happens to hold the hardcoded port. See ../.env.example.
 */

// Mirror vite.config.ts: process env wins, then the repo-root .env, then 5173.
function webPort(): number {
  let fromFile: string | undefined;
  try {
    const envText = readFileSync(join(process.cwd(), "..", ".env"), "utf-8");
    const match = envText.match(/^\s*FIELDDRAFT_WEB_PORT\s*=\s*(\d+)/m);
    fromFile = match?.[1];
  } catch {
    // no .env — fall back to the default below
  }
  const port = Number(process.env.FIELDDRAFT_WEB_PORT ?? fromFile);
  return Number.isInteger(port) && port > 0 ? port : 5173;
}

// Mirror vite.config.ts: HTTPS when the Tailscale cert is present, else HTTP.
const certDir = join(homedir(), ".local/share/tailscale-certs");
const hasTailscaleCert =
  existsSync(join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.crt")) &&
  existsSync(join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.key"));

const scheme = hasTailscaleCert ? "https" : "http";
const baseURL = `${scheme}://localhost:${webPort()}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  timeout: 30_000,
  expect: { timeout: 5_000 },
  use: {
    baseURL,
    // The Tailscale cert is issued for the *.ts.net host, not `localhost`, so a
    // localhost HTTPS connection trips a SAN mismatch — harmless for local e2e.
    ignoreHTTPSErrors: true,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium-webgpu",
      use: {
        ...devices["Desktop Chrome"],
        // Real GPU only — no swiftshader fallback. Playwright launches
        // a visible browser window so the test browser hits the host's
        // actual graphics stack. WebGPU is enabled by default in
        // current Chrome builds; the only flag we still pass widens
        // adapter discovery on Linux (Mesa, NVidia, AMD).
        headless: false,
        launchOptions: {
          args: ["--enable-features=Vulkan,WebGPU"],
        },
      },
    },
  ],
  webServer: {
    command: "npm run dev",
    url: baseURL,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    ignoreHTTPSErrors: true,
  },
});
