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

    // Read the committed circles (centre + radius). Everything below is built
    // in SKETCH space and projected, because that is the space the marquee
    // itself works in: `selectTool` keeps the drag corners in sketch mm, tests
    // containment with an axis-aligned box in sketch mm, and even decides
    // enclosing-vs-crossing from the sketch-u direction. A marquee laid out in
    // screen pixels (what this used to do) is a rotated quadrilateral over
    // there — its box contained nothing, and its direction could even flip.
    const circles = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      return calls
        .filter((c) => c.type === "sketch_circle")
        .map((c) => ({
          centre: c.params?.center as [number, number],
          radius: c.params?.radius as number,
        }));
    });
    expect(circles.length).toBe(3);

    circles.sort((a, b) => a.centre[0] - b.centre[0]);
    const [left, middle, right] = circles;
    const r = Math.max(left.radius, middle.radius, right.radius);
    const pad = r + 10;

    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);

    // ── Enclosing (LEFT→RIGHT in sketch u): a box that fully contains the LEFT
    // and MIDDLE circles and stops between MIDDLE and RIGHT.
    const u0 = left.centre[0] - pad;
    const u1 = (middle.centre[0] + right.centre[0]) / 2;
    const v0 = Math.min(left.centre[1], middle.centre[1]) - pad;
    const v1 = Math.max(left.centre[1], middle.centre[1]) + pad;
    expect(u1, "box must clear the middle circle").toBeGreaterThan(
      middle.centre[0] + r,
    );

    const [e0x, e0y] = await sketchToClientPx(page, u0, v0);
    const [e1x, e1y] = await sketchToClientPx(page, u1, v1);
    await page.mouse.move(e0x, e0y);
    await page.mouse.down();
    await page.mouse.move((e0x + e1x) / 2, (e0y + e1y) / 2, { steps: 5 });
    await page.mouse.move(e1x, e1y, { steps: 5 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    // Two circles enclosed: each contributes its centre point plus the circle
    // curve, so the selection holds 4 entities (2 circles + 2 centres).
    const enclosingSel = await sketchSelection(page);
    expect(enclosingSel.length).toBe(4);

    // Clear with a plain click on empty sketch space, left of the left circle.
    const [gapX, gapY] = await sketchToClientPx(
      page,
      left.centre[0] - r - 15,
      (v0 + v1) / 2,
    );
    await page.mouse.click(gapX, gapY);
    await page.waitForTimeout(150);
    expect((await sketchSelection(page)).length).toBe(0);

    // ── Crossing (RIGHT→LEFT in sketch u): a box that merely CLIPS the right
    // circle's rim — it starts clear of the circle and drags back onto it.
    const [c0x, c0y] = await sketchToClientPx(
      page,
      right.centre[0] + r + 20,
      right.centre[1] - r - 20,
    );
    const [c1x, c1y] = await sketchToClientPx(
      page,
      right.centre[0] + r - 5,
      right.centre[1] + r + 20,
    );
    await page.mouse.move(c0x, c0y);
    await page.mouse.down();
    await page.mouse.move(c1x, c1y, { steps: 8 });
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
