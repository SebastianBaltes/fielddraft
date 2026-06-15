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
 * Interactive offset (F15). Offset isn't a kernel feature tool — it's an
 * editing mode (like Trim/Fillet/Mirror): the user picks the Offset tool,
 * sets a distance, then clicks a curve ON THE SIDE they want the parallel
 * copy. A single click picks the nearest curve and appends an editable
 * concentric copy (circle/arc grow if the click is outside the rim, shrink
 * if inside; lines offset toward the clicked side). See
 * `web/src/sketchOffset.ts` + store.offsetSketchAt.
 *
 * The offset geometry is exhaustively unit-tested; this E2E proves the
 * interactive path: draw a circle, pick Offset, click clearly OUTSIDE the
 * rim → the session and committed sketch each hold TWO circles, and the new
 * circle's radius is larger than the original by ~the offset distance. Cold
 * WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch offset", () => {
  test("offsetting a circle on the outside appends a larger concentric copy", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    // Draw a circle centred at the canvas centre with a small radius.
    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 60, cy, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const circleCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_circle").length;

    expect(await circleCount()).toBe(1);

    // Read the committed circle's solved centre + radius to aim the click.
    const orig = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const c = calls.find((s) => s.type === "sketch_circle");
      const ctr = (c?.params?.center ?? null) as [number, number] | null;
      const r = (c?.params?.radius ?? null) as number | null;
      if (!ctr || r == null) return null;
      return { ctr, r };
    });
    expect(orig).not.toBeNull();

    // Pick Offset first — the distance field is now an in-scene input shown
    // only while the tool is active (ticket pqoe) — then set a known offset
    // distance.
    const distance = 5;
    await page.getByTestId("sketch-tool-sketch_offset").click();
    await page.getByTestId("sketch-offset-distance").fill(String(distance));
    await settleSketchCamera(page);

    // Click clearly OUTSIDE the rim: a point along +x at radius +
    // (distance/2) so it's near enough to pick the rim but plainly on the
    // outside, selecting the grow side.

    const clickWorld: [number, number] = [
      orig!.ctr[0] + orig!.r + distance / 2,
      orig!.ctr[1],
    ];
    const [px, py] = await sketchToClientPx(page, clickWorld[0], clickWorld[1]);
    await page.mouse.click(px, py);
    await page.waitForTimeout(400);

    expect(await circleCount()).toBe(2);

    // The new circle's radius differs from the original by ~the distance.
    const radii = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      return calls
        .filter((s) => s.type === "sketch_circle")
        .map((s) => (s.params?.radius ?? null) as number | null);
    });
    expect(radii.length).toBe(2);
    const newRadius = radii.find((r) => r != null && Math.abs(r - orig!.r) > 1e-6);
    expect(newRadius).not.toBeUndefined();
    expect(Math.abs(newRadius! - orig!.r)).toBeCloseTo(distance, 1);
    // Outside click → grow.
    expect(newRadius!).toBeGreaterThan(orig!.r);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_circle").length).toBe(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
