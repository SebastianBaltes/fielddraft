import { expect, test } from "@playwright/test";
import {
  pickSketchTool,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
} from "./sketchHarness";

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
    await pickSketchTool(page, "sketch_line");
    await settleSketchCamera(page);

    // Draw the line by dragging between two SKETCH points (deliberately
    // off-axis, so the tool infers no horizontal/vertical constraint).
    const a: [number, number] = [-30, -20];
    const b: [number, number] = [30, 20];
    const [ax, ay] = await sketchToClientPx(page, a[0], a[1]);
    const [bx, by] = await sketchToClientPx(page, b[0], b[1]);
    await page.mouse.move(ax, ay);
    await page.mouse.down();
    await page.mouse.move((ax + bx) / 2, (ay + by) / 2, { steps: 5 });
    await page.mouse.move(bx, by, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(200);
    expect(await subCallTypes(page)).toContain("sketch_line");

    // The length editor opens by CLICKING THE PLACED LINE'S DIMENSION READOUT.
    // There is no live length/angle HUD on the rubber-band any more — this test
    // used to click-start a pending line, move the cursor and wait for
    // `sketch-live-length` + `sketch-live-angle` to appear; the angle field no
    // longer exists at all, and the length field only shows for the readout of
    // the most recently placed line (`lineTool` -> `openLineLengthEditor`).
    //
    // The readout sits at the line's screen midpoint, pushed out along the
    // screen normal by LABEL_OFFSET_PX = 18 (see `sketchCanvasHud.ts`); the hit
    // radius is 24 px.
    const mx = (ax + bx) / 2;
    const my = (ay + by) / 2;
    const dx = bx - ax;
    const dy = by - ay;
    const screenLen = Math.hypot(dx, dy);
    const anchorX = mx + (dy / screenLen) * 18;
    const anchorY = my + (-dx / screenLen) * 18;
    await page.mouse.click(anchorX, anchorY);

    const lengthField = page.getByTestId("sketch-live-length");
    await expect(lengthField).toBeVisible();

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
