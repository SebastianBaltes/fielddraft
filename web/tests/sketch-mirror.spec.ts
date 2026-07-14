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
 * Interactive mirror (F14). Mirror isn't a kernel feature tool — it's an
 * editing mode (like Trim/Fillet). Tickets jq6i/ps21 turned it into the
 * Onshape TWO-STEP flow (`sketchTools/mirrorTool.ts`): the first click picks
 * the AXIS LINE, and every click after that mirrors the entity under the
 * cursor across it, live. There is no select-first step any more — picking the
 * axis explicitly CLEARS the selection — so a test that pre-selects and then
 * only clicks the axis mirrors nothing at all.
 *
 * The reflection geometry is exhaustively unit-tested; this E2E proves the
 * interactive path: draw a circle on the LEFT and a vertical axis line in the
 * MIDDLE, pick Mirror, click the axis, then click the circle's RIM (the circle
 * hit-test matches a click at centre-distance ≈ radius) → the session and the
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
    await pickSketchTool(page, "sketch_circle");
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

    // Read the solved circle (centre + radius) and the axis line, so both
    // clicks below land on real geometry rather than on remembered pixels.
    const geom = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const c = calls.find((s) => s.type === "sketch_circle");
      const l = calls.find((s) => s.type === "sketch_line");
      const centre = (c?.params?.center ?? null) as [number, number] | null;
      const radius = (c?.params?.radius ?? null) as number | null;
      const a = (l?.params?.start ?? null) as [number, number] | null;
      const b = (l?.params?.end ?? null) as [number, number] | null;
      if (!centre || radius === null || !a || !b) return null;
      return {
        rim: [centre[0] + radius, centre[1]] as [number, number],
        axisMid: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2] as [number, number],
      };
    });
    expect(geom, "solved circle + axis line").not.toBeNull();

    await pickSketchTool(page, "sketch_mirror");
    await settleSketchCamera(page);

    // Step 1: the axis.
    const [amx, amy] = await sketchToClientPx(
      page,
      geom!.axisMid[0],
      geom!.axisMid[1],
    );
    await page.mouse.click(amx, amy);
    await page.waitForTimeout(200);

    // Step 2: the part to mirror — its rim, not its centre.
    const [crx, cry] = await sketchToClientPx(page, geom!.rim[0], geom!.rim[1]);
    await page.mouse.click(crx, cry);
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
