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
 * Interactive mirror (F14). Mirror isn't a kernel feature tool — it's an
 * editing mode (like Trim/Fillet): the user selects source geometry, picks
 * the Mirror tool, then clicks an axis LINE; every selected simple-geometry
 * sub-call (line/circle/arc) is reflected across that line and appended as an
 * editable copy (see `web/src/sketchMirror.ts` + store.mirrorSketchSelection).
 *
 * The reflection geometry is exhaustively unit-tested; this E2E proves the
 * interactive path: draw a circle on the LEFT and a vertical axis line in the
 * MIDDLE, select the circle, mirror across the axis → the session and the
 * committed sketch each hold TWO circles. Cold WebGPU first-paint is slow, so
 * the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch mirror", () => {
  test("mirroring a selected circle across an axis line copies it", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    // Circle on the LEFT: centre at cx-100, dragged out to a small radius.
    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.mouse.move(cx - 100, cy);
    await page.mouse.down();
    await page.mouse.move(cx - 70, cy, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    // Vertical axis line down the MIDDLE.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx, cy - 100);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 4 });
    await page.mouse.move(cx, cy + 100, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const circleCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_circle").length;

    expect(await circleCount()).toBe(1);

    // SELECT mode: pick the circle by clicking its solved centre. Read the
    // committed centre coord and project it to a client pixel at click time.
    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);

    const circleCentre = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const c = calls.find((s) => s.type === "sketch_circle");
      const ctr = (c?.params?.center ?? null) as [number, number] | null;
      return ctr;
    });
    expect(circleCentre).not.toBeNull();
    const [ccx, ccy] = await sketchToClientPx(
      page,
      circleCentre![0],
      circleCentre![1],
    );
    await page.mouse.click(ccx, ccy);
    await page.waitForTimeout(200);

    const selection = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(selection.length).toBeGreaterThan(0);

    // Mirror: pick the tool (keeps the selection), then click the axis line
    // at its MIDPOINT (projected from the solved line endpoints).
    await page.getByTestId("sketch-tool-sketch_mirror").click();
    await settleSketchCamera(page);

    const axisMid = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const l = calls.find((s) => s.type === "sketch_line");
      const a = (l?.params?.start ?? null) as [number, number] | null;
      const b = (l?.params?.end ?? null) as [number, number] | null;
      if (!a || !b) return null;
      return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2] as [number, number];
    });
    expect(axisMid).not.toBeNull();
    const [amx, amy] = await sketchToClientPx(page, axisMid![0], axisMid![1]);
    await page.mouse.click(amx, amy);
    await page.waitForTimeout(400);

    expect(await circleCount()).toBe(2);

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
