import { expect, test } from "@playwright/test";

/**
 * F18: live numeric input while drawing a LINE ("type-while-drawing").
 *
 * INTERACTION IMPLEMENTED — CLICK-CLICK (pending line):
 *   1. Pick the line tool.
 *   2. A single CLICK on empty canvas (no drag) starts a PENDING line that
 *      rubber-bands to the cursor with no button held; a small HUD appears
 *      at the line midpoint with a LENGTH (`sketch-live-length`) and an
 *      ANGLE (`sketch-live-angle`) field, the length field auto-focused.
 *   3. Moving the cursor updates the HUD's live measurement.
 *   4. Typing an exact length and pressing Enter commits a precise line
 *      PLUS a DRIVEN `sketch_constraint_distance` so the line stays
 *      parametric. (A plain DRAG still commits a free line, unchanged.)
 *
 * This spec drives the click-then-type path: click to start, move to give a
 * live direction, type 50 into the length field, press Enter, then assert the
 * committed bundle is a `sketch_line` + a driven distance of ≈50 mm and the
 * line's measured endpoint length is ≈50 mm.
 *
 * Cold WebGPU first-paint can be slow → generous timeout.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch live numeric input (line)", () => {
  test("type a length + Enter commits a line with a driven distance", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    // Click (no drag) to start the pending line in the positive quadrant.
    const startSx = cx + 80;
    const startSy = cy + 80;
    await page.mouse.move(startSx, startSy);
    await page.mouse.down();
    await page.mouse.up();

    // Move the cursor to the right so the live direction ≈ 0° — the typed
    // length will then commit roughly horizontally. (Direction is only a
    // fallback; we type the length, leave the angle to the live value.)
    await page.mouse.move(startSx + 120, startSy, { steps: 6 });

    // The HUD should be visible with the length field focused.
    const lengthField = page.getByTestId("sketch-live-length");
    const angleField = page.getByTestId("sketch-live-angle");
    await expect(lengthField).toBeVisible();
    await expect(angleField).toBeVisible();

    // Type an exact length and commit with Enter.
    await lengthField.click();
    await lengthField.fill("50");
    await lengthField.press("Enter");

    await page.waitForTimeout(500);

    // The session spine must now hold the line + a driven distance.
    const sub = await page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            state: {
              sketchSession: {
                subCalls: Array<{
                  type: string;
                  params?: Record<string, unknown>;
                }>;
              };
            };
          };
        }
      ).__fdStore;
      return store?.state.sketchSession.subCalls ?? [];
    });

    const types = sub.map((c) => c.type);
    expect(types).toContain("sketch_line");
    expect(types).toContain("sketch_constraint_distance");

    const dist = sub.find((c) => c.type === "sketch_constraint_distance");
    expect(dist).toBeTruthy();
    expect(dist!.params!.driven).toBe(true);
    expect(Number(dist!.params!.value_mm)).toBeCloseTo(50, 1);

    // The committed line's endpoints must span ≈50 mm.
    const line = sub.find((c) => c.type === "sketch_line");
    expect(line).toBeTruthy();
    const start = line!.params!.start as [number, number];
    const end = line!.params!.end as [number, number];
    const len = Math.hypot(end[0] - start[0], end[1] - start[1]);
    expect(len).toBeCloseTo(50, 1);

    // Finish the sketch — the bundle survives into the committed feature.
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{ type: string }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id).map((c) => c.type);
    });
    expect(committed).not.toBeNull();
    expect(committed!).toContain("sketch_line");
    expect(committed!).toContain("sketch_constraint_distance");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
