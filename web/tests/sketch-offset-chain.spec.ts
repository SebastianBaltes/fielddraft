import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * F15-full: chain / contour offset. Unlike the single-curve offset
 * (sketch-offset.spec.ts), clicking the Offset tool on a curve that is
 * part of a CONNECTED chain offsets the WHOLE chain toward the clicked
 * side as one connected set of editable sub-calls — exactly like
 * Onshape / FreeCAD "Offset" (see web/src/sketchOffset.ts `offsetChain`
 * + store.offsetSketchAt).
 *
 * The chain geometry is exhaustively unit-tested; this E2E proves the
 * interactive path: draw a connected open 3-line polyline (the segments
 * share endpoints via draw-time snapping), pick Offset, set a distance,
 * click ONE segment toward the inside, and assert the session gains MORE
 * THAN ONE offset `sketch_line` (the whole chain, not just the clicked
 * segment). Then Finish and assert the offset survives into the committed
 * sketch. Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch chain offset", () => {
  test("offsetting one segment of a connected chain offsets the whole chain", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    const drawLine = async (x1: number, y1: number, x2: number, y2: number) => {
      await page.getByTestId("sketch-tool-sketch_line").click();
      await page.mouse.move(x1, y1);
      await page.mouse.down();
      await page.mouse.move((x1 + x2) / 2, (y1 + y2) / 2, { steps: 4 });
      await page.mouse.move(x2, y2, { steps: 8 });
      await page.mouse.up();
      await page.waitForTimeout(150);
    };

    // A connected open 3-segment "staircase" polyline; each segment starts
    // at the previous segment's end so endpoint snapping keeps them
    // coincident:
    //   A(cx-120,cy) → B(cx,cy) → C(cx,cy-120) → D(cx+120,cy-120)
    await drawLine(cx - 120, cy, cx, cy);
    await drawLine(cx, cy, cx, cy - 120);
    await drawLine(cx, cy - 120, cx + 120, cy - 120);

    const lineCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_line").length;
    expect(await lineCount()).toBe(3);

    // Pick the Offset tool, then set a known distance — the distance field
    // is now an in-scene input shown only while the tool is active (ticket
    // pqoe).
    const distance = 8;
    await page.getByTestId("sketch-tool-sketch_offset").click();
    await page.getByTestId("sketch-offset-distance").fill(String(distance));
    await settleSketchCamera(page);

    // Click the FIRST (bottom) segment a little ABOVE it — toward the
    // inside of the staircase. The click both picks the segment and selects
    // the offset side; the whole connected chain must offset on that side.
    const clickWorld = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const first = calls.find((c) => c.type === "sketch_line");
      const s = (first?.params?.start ?? null) as [number, number] | null;
      const e = (first?.params?.end ?? null) as [number, number] | null;
      if (!s || !e) return null;
      // Midpoint, nudged toward +y (into the staircase interior).
      return [(s[0] + e[0]) / 2, (s[1] + e[1]) / 2 + 3] as [number, number];
    });
    expect(clickWorld).not.toBeNull();

    const [px, py] = await sketchToClientPx(page, clickWorld![0], clickWorld![1]);
    await page.mouse.click(px, py);
    await page.waitForTimeout(400);

    // The whole chain offset: the original 3 lines plus MORE THAN ONE new
    // offset line (a single-segment offset would add just one).
    const after = await lineCount();
    expect(
      after,
      `expected the chain offset to add more than one line (got ${after} total)`,
    ).toBeGreaterThan(4);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeVisible();

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_line").length).toBe(after);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
