import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright config for FieldDraft smoke and viewport tests.
 *
 * We only run Chromium with WebGPU enabled — WebGPU is a hard requirement,
 * no fallback. Firefox/Safari will land once their WebGPU support is stable
 * in CI-grade headless mode.
 */
export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  // Real-GPU runs average ~33 s/test (WebGPU shader compile + first-paint),
  // so a 30 s cap sat right on the edge — under load, tests randomly crossed
  // it and failed with `mouse.move: Test timeout` (page unresponsive, not a
  // logic bug). 60 s gives the compile headroom; the suite is still serial.
  timeout: 60_000,
  expect: { timeout: 5_000 },
  use: {
    baseURL: "http://localhost:5173",
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
    url: "http://localhost:5173",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
