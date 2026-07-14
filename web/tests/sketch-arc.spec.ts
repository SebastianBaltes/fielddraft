import { expect, test } from "@playwright/test";
import { expandGeometrySection } from "./sketchHarness";

/**
 * Interactive arc tool (UI gap #1). The arc is a three-click
 * centre → start → end gesture:
 *   1. centre, 2. start point (fixes radius + start angle),
 *   3. end point (fixes the sweep).
 * Here we drive the three clicks on the 2D overlay and assert the
 * committed sketch holds a single `sketch_arc` sub-call with a sane
 * radius and a non-zero sweep. Cold WebGPU first-paint can be slow, so
 * the per-test timeout is generous.
 */
test.describe.configure({ timeout: 60_000 });

test.describe("Sketch arc tool", () => {
  test("centre → start → end commits a sketch_arc", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_arc").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    // Centre in the positive quadrant; start 60 px to the right (start
    // angle ≈ 0); end 60 px above (screen-up = sketch +y), i.e. a ~+90°
    // CCW sweep.
    const centerSx = cx + 96;
    const centerSy = cy + 96;
    const startSx = centerSx + 60;
    const startSy = centerSy;
    const endSx = centerSx;
    const endSy = centerSy - 60;

    await page.mouse.click(centerSx, centerSy);
    await page.mouse.move(startSx, startSy, { steps: 4 });
    await page.mouse.click(startSx, startSy);
    await page.mouse.move(endSx, endSy, { steps: 8 });
    await page.mouse.click(endSx, endSy);

    // One committed sub-call (the arc). The geometry list lives inside the
    // collapsed-by-default "Geometry & Constraints" sub-accordion (ticket 87a1).
    await expandGeometrySection(page);
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    await page.waitForTimeout(700);
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!.length).toBe(1);
    const arc = committed![0] as {
      type: string;
      params: {
        center: [number, number];
        radius: number;
        start_angle: number;
        sweep_angle: number;
      };
    };
    expect(arc.type).toBe("sketch_arc");
    expect(arc.params.radius).toBeGreaterThan(10);
    // Centre is in the positive-x sketch quadrant.
    expect(arc.params.center[0]).toBeGreaterThan(0);
    // A real, positive (CCW) sweep was captured — not a degenerate zero.
    expect(Math.abs(arc.params.sweep_angle)).toBeGreaterThan(0.3);
    expect(arc.params.sweep_angle).toBeGreaterThan(0);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
