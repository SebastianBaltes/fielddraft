import { expect, test } from "@playwright/test";

/**
 * Mobile responsive layout (phone width). Below the 768px breakpoint the
 * outliner is an off-canvas drawer: parked off the right edge so the
 * viewport owns the full width, slid in by a topbar toggle, dismissed by
 * tapping the scrim.
 */
test.describe("Mobile responsive layout", () => {
  const VW = 390;
  test.use({ viewport: { width: VW, height: 844 } });

  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
    await page.waitForTimeout(600);
  });

  test("outliner becomes a topbar-toggled drawer; viewport keeps full width", async ({
    page,
  }) => {
    const toggle = page.getByTestId("mobile-nav-toggle");
    const outliner = page.getByTestId("fielddraft-outliner");
    const canvas = page.getByTestId("fielddraft-canvas");

    // Toggle appears only at phone width.
    await expect(toggle).toBeVisible();

    // Closed: drawer parked off the right edge; viewport spans full width.
    let box = (await outliner.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(VW - 1);
    const canvasBox = (await canvas.boundingBox())!;
    expect(canvasBox.width).toBeGreaterThan(VW - 24);

    // Open: tap the toggle, drawer slides in flush against the right edge.
    await toggle.click();
    await page.waitForTimeout(150); // React state flush (no CSS transition)
    box = (await outliner.boundingBox())!;
    expect(box.x).toBeLessThan(VW);
    expect(box.x + box.width).toBeLessThanOrEqual(VW + 1);
    await expect(page.getByTestId("mobile-nav-scrim")).toBeVisible();

    // Close: tap the scrim.
    await page
      .getByTestId("mobile-nav-scrim")
      .click({ position: { x: 5, y: 200 } });
    await page.waitForTimeout(150);
    box = (await outliner.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(VW - 1);
  });

  test("the 44px touch target is honoured", async ({ page }) => {
    const box = (await page.getByTestId("mobile-nav-toggle").boundingBox())!;
    expect(box.width).toBeGreaterThanOrEqual(44);
    expect(box.height).toBeGreaterThanOrEqual(44);
  });

  test("toolbar buttons and bottom-tab controls meet the 44px touch target", async ({
    page,
  }) => {
    // On a phone the tool row is collapsed behind a "Tools" toggle (ticket
    // apr3): `.toolbar-tools` is `display:none` until the menu opens, so the
    // measure button inside it has NO box yet. The toggle itself is the visible
    // touch target — check it, then open the menu and check the revealed tool.
    const toolsToggle = (await page
      .getByTestId("toolbar-menu-toggle")
      .boundingBox())!;
    expect(toolsToggle.height).toBeGreaterThanOrEqual(44);

    await page.getByTestId("toolbar-menu-toggle").click();
    await page.waitForTimeout(150);
    const tool = (await page.getByTestId("measure-toggle").boundingBox())!;
    expect(tool.height).toBeGreaterThanOrEqual(44);
    await page.getByTestId("toolbar-menu-toggle").click(); // close again

    // Bottom strip: add-tab button is a square ≥44px.
    const add = (await page.getByTestId("tab-add").boundingBox())!;
    expect(add.width).toBeGreaterThanOrEqual(44);
    expect(add.height).toBeGreaterThanOrEqual(44);

    // The whole tab row is the tap target (≥44 tall). The caret inside it is
    // decorative, not an independent target, so don't hold it to 44px.
    const tab = (await page
      .locator(".fd-bottom-tabs .tab")
      .first()
      .boundingBox())!;
    expect(tab.height).toBeGreaterThanOrEqual(44);
  });
});

test.describe("Desktop layout keeps the outliner docked", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("no mobile toggle; outliner sits inside the viewport width", async ({
    page,
  }) => {
    await page.goto("/");
    await page.waitForTimeout(400);
    await expect(page.getByTestId("mobile-nav-toggle")).toBeHidden();
    const box = (await page.getByTestId("fielddraft-outliner").boundingBox())!;
    // Docked at the right, fully on-screen.
    expect(box.x).toBeGreaterThan(0);
    expect(box.x + box.width).toBeLessThanOrEqual(1281);
  });

  test("toolbar buttons keep their compact desktop height", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(400);
    // The mobile 44px up-size must stay scoped to the media query: on desktop
    // the Measure button keeps its capped (~40px) height.
    const tool = (await page.getByTestId("measure-toggle").boundingBox())!;
    expect(tool.height).toBeLessThan(44);
  });
});
