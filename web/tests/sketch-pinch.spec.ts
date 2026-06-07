import { expect, test } from "@playwright/test";

/**
 * Two-finger pinch-to-zoom on the 2D sketch overlay. The overlay captures
 * touches for sketching, so OrbitControls never sees them — without this
 * gesture a touch user cannot zoom the sketch at all. The gesture is driven
 * by synthesising wheel events onto the WebGPU canvas (the same dolly path
 * the mouse wheel uses); here we assert the orthographic camera zoom moves
 * in the right direction for a spread (in) and a squeeze (out).
 *
 * Multi-touch isn't expressible through Playwright's single-point
 * `touchscreen`, so we drive raw touches via CDP `Input.dispatchTouchEvent`
 * (which the browser turns into `pointerType: "touch"` pointer events).
 */
test.use({ hasTouch: true, viewport: { width: 900, height: 700 } });

// A cold WebGPU first-paint (WASM init + shader compile) can take ~25s, so
// the default 30s is too tight when this spec runs first/alone. In the full
// suite it runs warm and finishes in a few seconds.
test.describe.configure({ timeout: 60_000 });

async function enterSketch(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.waitForTimeout(600);
  await page.evaluate(() => {
    const store = (
      window as unknown as { __fdStore?: { enterSketch: (p: string) => void } }
    ).__fdStore;
    if (!store) throw new Error("__fdStore not exposed");
    store.enterSketch("qCreatedBy(origin_plane_xy)");
  });
  const canvas = page.getByTestId("sketch-canvas-2d");
  await expect(canvas).toBeVisible();
  return (await canvas.boundingBox())!;
}

function readZoom(page: import("@playwright/test").Page) {
  return page.evaluate(
    () =>
      (
        window as unknown as { __fdSketchCamera?: () => { zoom: number } }
      ).__fdSketchCamera!().zoom,
  );
}

test.describe("Sketch pinch-to-zoom", () => {
  test("spreading two fingers zooms the sketch camera in", async ({ page }) => {
    const box = await enterSketch(page);
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    const zoomBefore = await readZoom(page);

    const client = await page.context().newCDPSession(page);
    const points = (sep: number) => [
      { x: cx - sep, y: cy, id: 1 },
      { x: cx + sep, y: cy, id: 2 },
    ];
    await client.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: points(20),
    });
    for (let sep = 50; sep <= 200; sep += 30) {
      await client.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: points(sep),
      });
      await page.waitForTimeout(20);
    }
    await client.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    await page.waitForTimeout(120);

    const zoomAfter = await readZoom(page);
    expect(zoomAfter).toBeGreaterThan(zoomBefore * 1.1);
  });

  test("squeezing two fingers zooms the sketch camera out", async ({ page }) => {
    const box = await enterSketch(page);
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    const zoomBefore = await readZoom(page);

    const client = await page.context().newCDPSession(page);
    const points = (sep: number) => [
      { x: cx - sep, y: cy, id: 1 },
      { x: cx + sep, y: cy, id: 2 },
    ];
    await client.send("Input.dispatchTouchEvent", {
      type: "touchStart",
      touchPoints: points(200),
    });
    for (let sep = 170; sep >= 30; sep -= 30) {
      await client.send("Input.dispatchTouchEvent", {
        type: "touchMove",
        touchPoints: points(sep),
      });
      await page.waitForTimeout(20);
    }
    await client.send("Input.dispatchTouchEvent", {
      type: "touchEnd",
      touchPoints: [],
    });
    await page.waitForTimeout(120);

    const zoomAfter = await readZoom(page);
    expect(zoomAfter).toBeLessThan(zoomBefore * 0.9);
  });
});
