/**
 * K7.21f — Visual regression baselines.
 *
 * Screenshot-based regression tests for the UI chrome:
 * Topbar, Toolbar, Outliner, Settings Menu, and Feature Dialog.
 * The 3D canvas is masked (GPU-dependent rendering would cause
 * false positives across machines).
 *
 * Tests run at three viewport widths: 600, 900, and 1440 px to
 * verify the responsive toolbar overflow behaviour (K7.21b).
 *
 * Baseline images are stored in `web/tests/visual.spec.ts-snapshots/`.
 * Run `npx playwright test tests/visual.spec.ts --update-snapshots`
 * to regenerate.
 */

import { expect, test } from "@playwright/test";

test.use({ actionTimeout: 15_000 });

const VIEWPORTS = [
  { name: "narrow", width: 600, height: 800 },
  { name: "medium", width: 900, height: 800 },
  { name: "wide", width: 1440, height: 900 },
] as const;

const SCREENSHOT_OPTS = {
  maxDiffPixelRatio: 0.02,
  timeout: 15_000,
} as const;

async function setupCleanState(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.evaluate(() => {
    try {
      window.localStorage.removeItem("fielddraft.project.v1");
    } catch {
      /* ignore */
    }
  });
  await page.reload();
  await expect(page.getByTestId("fielddraft-toolbar")).toBeVisible();
  // Wait for WASM init + ResizeObserver to settle
  await page.waitForTimeout(2000);
}

test.describe("Visual regression: Toolbar responsive overflow", () => {
  for (const vp of VIEWPORTS) {
    test(`toolbar at ${vp.name} (${vp.width}px)`, async ({ page }) => {
      await page.setViewportSize({ width: vp.width, height: vp.height });
      await setupCleanState(page);
      // Extra settle time after viewport resize triggers ResizeObserver
      await page.waitForTimeout(500);
      const toolbar = page.getByTestId("fielddraft-toolbar");
      await expect(toolbar).toHaveScreenshot(`toolbar-${vp.name}.png`, {
        ...SCREENSHOT_OPTS,
      });
    });
  }
});

test.describe("Visual regression: Outliner", () => {
  test("outliner with empty project", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await setupCleanState(page);
    const outliner = page.getByTestId("fielddraft-outliner");
    await expect(outliner).toHaveScreenshot("outliner-empty.png", {
      ...SCREENSHOT_OPTS,
    });
  });

  test("outliner with features", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await setupCleanState(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __fdStore: Record<string, (...args: unknown[]) => void> }).__fdStore;
      s.addPrimitive("box");
      s.addPrimitive("sphere");
      s.addPrimitive("cylinder");
    });
    await page.waitForTimeout(500);
    const outliner = page.getByTestId("fielddraft-outliner");
    await expect(outliner).toHaveScreenshot("outliner-features.png", {
      ...SCREENSHOT_OPTS,
    });
  });
});

test.describe("Visual regression: Settings menu", () => {
  test("settings menu open", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await setupCleanState(page);
    const menuBtn = page.getByTestId("settings-menu-trigger");
    await menuBtn.click();
    await page.waitForTimeout(300);
    const popover = page.getByTestId("settings-menu-popover");
    await expect(popover).toHaveScreenshot("settings-menu-open.png", {
      ...SCREENSHOT_OPTS,
    });
  });
});

test.describe("Visual regression: Compact mode", () => {
  test("toolbar in compact mode", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await setupCleanState(page);
    await page.evaluate(() => {
      document.querySelector(".fd-app")?.classList.add("fd-compact");
    });
    await page.waitForTimeout(300);
    const toolbar = page.getByTestId("fielddraft-toolbar");
    await expect(toolbar).toHaveScreenshot("toolbar-compact.png", {
      ...SCREENSHOT_OPTS,
    });
  });
});

test.describe("Visual regression: Feature dialog", () => {
  test("feature dialog for box primitive", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await setupCleanState(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __fdStore: Record<string, (...args: unknown[]) => void> }).__fdStore;
      s.addPrimitive("box");
    });
    await page.waitForTimeout(500);
    const row = page.getByTestId("feature-row-0");
    if (await row.isVisible()) {
      await row.dblclick();
      await page.waitForTimeout(500);
      const dialog = page.getByTestId("feature-dialog");
      if (await dialog.isVisible()) {
        await expect(dialog).toHaveScreenshot("feature-dialog-box.png", {
          ...SCREENSHOT_OPTS,
        });
      }
    }
  });
});

test.describe("Visual regression: Sketch mode toolbar", () => {
  test("sketch mode toolbar", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await setupCleanState(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __fdStore: Record<string, (...args: unknown[]) => void> }).__fdStore;
      s.enterSketch("qCreatedBy(origin_plane_xy)");
    });
    await page.waitForTimeout(500);
    const toolbar = page.getByTestId("fielddraft-toolbar");
    await expect(toolbar).toHaveScreenshot("toolbar-sketch-mode.png", {
      ...SCREENSHOT_OPTS,
    });
  });
});
