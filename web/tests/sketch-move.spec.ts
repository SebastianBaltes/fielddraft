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

    // Move: base click then target click, offset to the RIGHT by ~80 px.
    await page.getByTestId("sketch-tool-sketch_move").click();
    await page.mouse.click(cx - 100, cy); // base
    await page.waitForTimeout(120);
    await page.mouse.click(cx - 20, cy); // target (+80 px in screen X)
    await page.waitForTimeout(400);

    // Still exactly one circle (in-place, no new geometry).
    expect(await circleCount()).toBe(1);

    const after = await firstCircleCentre(page);
    expect(after).not.toBeNull();
    // The centre moved to the right (+X in sketch space). Snapping can nudge
    // the magnitude, so just assert a meaningful positive shift in X.
    expect(after![0] - before![0]).toBeGreaterThan(20);
    expect(Math.abs(after![1] - before![1])).toBeLessThan(20);

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
