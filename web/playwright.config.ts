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
  // The TSL raymarcher is *very* slow under headless swiftshader (the
  // dev box has no real Vulkan device). Real-hardware runs are
  // sub-second; CI defaults of 30s are tight here.
  timeout: 120_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: "http://localhost:5173",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium-webgpu",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: {
          args: [
            "--enable-unsafe-webgpu",
            "--enable-features=Vulkan",
            "--use-vulkan=swiftshader",
            "--use-gl=angle",
            "--use-angle=swiftshader",
          ],
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
