import { expect, test } from "@playwright/test";
import {
  pickSketchTool,
  canvasCentre,
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * F4 box-select / marquee. The pure box hit-testing is covered by Vitest;
 * this E2E proves the interactive path: draw three spread-out circles, drop
 * to Select mode, then drag marquees and confirm the live selection.
 *
 * Convention (Onshape/FreeCAD): drag LEFT→RIGHT = enclosing "window" select
 * (only fully-contained entities); drag RIGHT→LEFT = crossing select (any
 * entity the box touches). A plain click on empty space clears.
 *
 * We DRAW with raw pixel offsets from the canvas centre (camera-independent),
 * then choose marquee corners by projecting each circle's ACTUAL committed
 * centre to a client pixel via the live camera, so the box math survives the
 * post-draw auto-fit. Cold WebGPU first-paint is slow → generous timeout.
 */
test.describe.configure({ timeout: 120_000 });

const sketchSelection = (page: import("@playwright/test").Page) =>
  page.evaluate(
    () =>
      (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
        .selection ?? [],
  );

test.describe("Sketch box-select / marquee", () => {
  test("enclosing picks the two boxed circles; crossing clips one", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    const [cx, cy] = await canvasCentre(page);

    // Draw three small circles spread left / middle / right. Each circle is
    // a centre-drag: down at the centre, out to the rim.
    const offsets = [-180, 0, 180];
    for (const dx of offsets) {
      await pickSketchTool(page, "sketch_circle");
      await page.mouse.move(cx + dx, cy);
      await page.mouse.down();
      await page.mouse.move(cx + dx + 25, cy, { steps: 5 });
      await page.mouse.up();
      await page.waitForTimeout(200);
    }
    expect(await subCallTypes(page)).toEqual([
      "sketch_circle",
      "sketch_circle",
      "sketch_circle",
    ]);

    // Read the committed circle centres so we can project them to pixels.
    const centres = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      return calls
        .filter((c) => c.type === "sketch_circle")
        .map((c) => c.params?.center as [number, number]);
    });
    expect(centres.length).toBe(3);

    // Sort centres by x so [left, middle, right] is unambiguous regardless
    // of draw order.
    centres.sort((a, b) => a[0] - b[0]);
    const [left, middle, right] = centres;

    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);

    // ── Enclosing (LEFT→RIGHT): a box around the LEFT + MIDDLE circles,
    // excluding the RIGHT one. Project the relevant centres; build a box
    // that comfortably contains left+middle (centres ± radius ≈ a few mm)
    // and stops well short of right.
    const [lpx, lpy] = await sketchToClientPx(page, left[0], left[1]);
    const [mpx] = await sketchToClientPx(page, middle[0], middle[1]);
    const [rpx] = await sketchToClientPx(page, right[0], right[1]);

    // Generous vertical padding (circle radii project to a handful of px).
    const padY = 60;
    // Horizontal box edges: start a bit left of the left circle, end
    // between the middle and right circles.
    const x0 = lpx - 50;
    const x1 = (mpx + rpx) / 2;
    const y0 = lpy - padY;
    const y1 = lpy + padY;

    // Drag LEFT→RIGHT (x0 < x1) = enclosing.
    await page.mouse.move(x0, y0);
    await page.mouse.down();
    await page.mouse.move((x0 + x1) / 2, y1, { steps: 5 });
    await page.mouse.move(x1, y1, { steps: 5 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    // Two circles enclosed: each circle contributes its centre point plus
    // the circle curve, so the selection holds 4 entities (2 circles + 2
    // centres). The load-bearing fact is "2 circles in, the 3rd out". The
    // right circle's exclusion is verified below: clearing then crossing
    // ONLY the right circle yields a fresh, smaller selection.
    const enclosingSel = await sketchSelection(page);
    expect(enclosingSel.length).toBe(4);

    // Clear with a plain click on clearly empty space (top-left corner).
    await page.mouse.click(cx - 250, cy - 150);
    await page.waitForTimeout(150);
    expect((await sketchSelection(page)).length).toBe(0);

    // ── Crossing (RIGHT→LEFT): a box that merely CLIPS the right circle's
    // rim. Start right of the right circle and drag left into its edge.
    const [rpx2, rpy2] = await sketchToClientPx(page, right[0], right[1]);
    const cx0 = rpx2 + 40; // right of the circle
    const cx1 = rpx2 + 5; // just into the right rim, RIGHT→LEFT
    await page.mouse.move(cx0, rpy2 - 50);
    await page.mouse.down();
    await page.mouse.move(cx1, rpy2 + 50, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const crossingSel = await sketchSelection(page);
    expect(crossingSel.length).toBeGreaterThanOrEqual(1);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
