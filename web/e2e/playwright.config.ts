import { defineConfig } from "@playwright/test";
import base from "../playwright.config";

/**
 * Headful-WebGPU e2e config for the specs in `web/e2e/` — visual acceptance
 * anchors that need a REAL GPU (run against DISPLAY=:1 with the mutter
 * Xwayland auth cookie, see memory "headful visual loop").
 *
 * Reuses the per-instance base config (port from FIELDDRAFT_WEB_PORT/.env,
 * https iff Tailscale cert) and only rebases testDir onto this directory.
 * Run from `web/`:
 *
 *   DISPLAY=:1 XAUTHORITY=/run/user/1000/.mutter-Xwaylandauth.<cookie> \
 *     npx playwright test --config e2e/playwright.config.ts
 */
export default defineConfig({
  ...base,
  testDir: ".",
  // The base webServer command (`npm run dev`) must run in `web/`, not in
  // `web/e2e/` — webServer.cwd defaults to the config file's directory.
  webServer: base.webServer
    ? { ...(base.webServer as object), cwd: ".." }
    : undefined,
});
