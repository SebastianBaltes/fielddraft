import { expect, test } from "@playwright/test";
import { expandGeometrySection } from "./sketchHarness";

/**
 * Construction geometry (UI gap #5). A drawn element can be flagged as
 * construction from its sub-spine row: it still solves and can be
 * referenced, but the kernel drops it from the extrudable region and the
 * canvas draws it dashed. Here we draw a circle, toggle it to
 * construction, and assert the committed sub-call carries
 * `construction: true`. Cold WebGPU first-paint is slow, so the per-test
 * timeout is generous.
 */
test.describe.configure({ timeout: 60_000 });

test.describe("Sketch construction geometry", () => {
  test("toggling a circle to construction flags its sub-call", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    // Draw a circle via a centre→rim drag.
    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 30, cy, { steps: 4 });
    await page.mouse.move(cx + 70, cy, { steps: 8 });
    await page.mouse.up();

    // The geometry list + construction toggle live inside the collapsed-by-
    // default "Geometry & Constraints" sub-accordion (ticket 87a1).
    await expandGeometrySection(page);
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    // Toggle the circle to construction geometry.
    await page.getByTestId("sketch-sub-row-construction-0").click();
    await expect(subSpineRows.first()).toHaveClass(/is-construction/);

    await page.waitForTimeout(500);
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeVisible();

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
    const circle = committed!.find((c) => c.type === "sketch_circle");
    expect(circle).toBeDefined();
    expect(circle!.params.construction).toBe(true);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
