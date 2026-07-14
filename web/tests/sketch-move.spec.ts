import { expect, test } from "@playwright/test";
import {
  pickSketchTool,
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * F17 Move / Copy: the selection transform isn't a kernel feature tool —
 * it's an editing mode like Trim/Mirror/Pattern. The user selects geometry
 * (Select mode), picks Move or Copy, then clicks a BASE point and a TARGET
 * point: the selection is translated by (target − base), in place for Move
 * or as appended copies for Copy.
 *
 * The transform math is exhaustively unit-tested (sketchPattern /
 * sketchTransform). These E2Es prove the interactive two-click path on real
 * WebGPU: Move keeps exactly ONE circle but shifts its centre; Copy leaves
 * the original and appends a SECOND circle. Cold first-paint is slow, so the
 * timeout is generous. Deltas are asserted approximately (snapping).
 */
test.describe.configure({ timeout: 120_000 });

/** Read the live session's first circle centre (sketch mm), or null. */
async function firstCircleCentre(
  page: import("@playwright/test").Page,
): Promise<[number, number] | null> {
  return page.evaluate(() => {
    const calls =
      (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
        .subCalls ?? [];
    const c = calls.find((s) => s.type === "sketch_circle");
    return (c?.params?.center ?? null) as [number, number] | null;
  });
}

test.describe("Sketch move / copy", () => {
  test("Move translates the selected circle in place (still one circle)", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    // A single circle on the LEFT so the move target stays on-canvas.
    await pickSketchTool(page, "sketch_circle");
    await page.mouse.move(cx - 120, cy);
    await page.mouse.down();
    await page.mouse.move(cx - 95, cy, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const circleCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_circle").length;
    expect(await circleCount()).toBe(1);

    const before = await firstCircleCentre(page);
    expect(before).not.toBeNull();

    // SELECT mode: pick the circle by clicking its solved centre.
    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);
    const [bcx, bcy] = await sketchToClientPx(page, before![0], before![1]);
    await page.mouse.click(bcx, bcy);
    await page.waitForTimeout(200);

    const selection = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(selection.length).toBeGreaterThan(0);

    // Move: base click then target click, offset along the sketch's +u axis.
    //
    // Two constraints, both learned the hard way:
    //  - Derive BOTH clicks in SKETCH space and project them. Nudging the
    //    target by N screen pixels at constant screen-y (what this used to do)
    //    is not a pure +u move: since the sketch-in-3D rebuild the overlay
    //    projects through the real plane frame, screen-horizontal is not
    //    sketch-horizontal, and the supposedly untouched v drifted by 40 mm.
    //  - The BASE click must land on EMPTY SPACE. `moveTool` only records a
    //    base→target base point when the press hits nothing; a press ON an
    //    entity arms a direct click-drag instead, and a click that never drags
    //    moves nothing at all. So base off to the LEFT of the circle, clear of
    //    its rim — not on its centre.
    const dU = 40;
    const radius = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const c = calls.find((s) => s.type === "sketch_circle");
      return (c?.params?.radius ?? null) as number | null;
    });
    expect(radius).not.toBeNull();
    const baseU = before![0] - (radius! + 15);

    await pickSketchTool(page, "sketch_move");
    const [mbx, mby] = await sketchToClientPx(page, baseU, before![1]);
    await page.mouse.click(mbx, mby); // base: empty space left of the circle
    await page.waitForTimeout(120);
    const [mtx, mty] = await sketchToClientPx(page, baseU + dU, before![1]);
    await page.mouse.click(mtx, mty); // target: +dU along sketch u
    await page.waitForTimeout(400);

    // Still exactly one circle (in-place, no new geometry).
    expect(await circleCount()).toBe(1);

    const after = await firstCircleCentre(page);
    expect(after).not.toBeNull();
    // Moved along +u by roughly dU; v is untouched. Snapping can nudge the
    // magnitude, so allow generous slack rather than asserting dU exactly.
    expect(after![0] - before![0]).toBeGreaterThan(dU / 2);
    expect(Math.abs(after![1] - before![1])).toBeLessThan(dU / 4);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_circle").length).toBe(1);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("Copy appends a translated circle (now two circles)", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    await pickSketchTool(page, "sketch_circle");
    await page.mouse.move(cx - 120, cy);
    await page.mouse.down();
    await page.mouse.move(cx - 95, cy, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const circleCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_circle").length;
    expect(await circleCount()).toBe(1);

    const before = await firstCircleCentre(page);
    expect(before).not.toBeNull();

    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);
    const [bcx, bcy] = await sketchToClientPx(page, before![0], before![1]);
    await page.mouse.click(bcx, bcy);
    await page.waitForTimeout(200);

    const selection = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(selection.length).toBeGreaterThan(0);

    // Copy: base then target → originals stay + one appended copy.
    await page.getByTestId("sketch-tool-sketch_copy").click();
    await page.mouse.click(cx - 100, cy); // base
    await page.waitForTimeout(120);
    await page.mouse.click(cx - 20, cy); // target
    await page.waitForTimeout(400);

    expect(await circleCount()).toBe(2);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_circle").length).toBe(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
