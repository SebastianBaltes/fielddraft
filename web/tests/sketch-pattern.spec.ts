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
 * Interactive pattern (F16). Pattern isn't a kernel feature tool — it's an
 * editing mode (like Trim/Mirror/Offset): the user selects source geometry,
 * sets a count (+ spacing for linear / total angle for circular) and clicks
 * one of the Pattern buttons; every selected simple-geometry sub-call
 * (line/circle/arc) is repeated and appended as editable copies (see
 * `web/src/sketchPattern.ts` + store.linearPatternSketch).
 *
 * The repetition geometry is exhaustively unit-tested; this E2E proves the
 * interactive path: draw a circle, select it, set count=3 and click "Linear
 * X" → the session and the committed sketch each hold THREE circles. Cold
 * WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch pattern", () => {
  test("linear-patterning a selected circle appends count-1 copies", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    // A single circle on the LEFT (so its copies stay on-canvas to the right).
    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.mouse.move(cx - 120, cy);
    await page.mouse.down();
    await page.mouse.move(cx - 95, cy, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const circleCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_circle").length;

    expect(await circleCount()).toBe(1);

    // SELECT mode: pick the circle by clicking its solved centre.
    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);

    const circleCentre = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const c = calls.find((s) => s.type === "sketch_circle");
      return (c?.params?.center ?? null) as [number, number] | null;
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

    // Set count = 3, then click Linear X. No canvas gesture required — the
    // pattern acts on the current selection.
    const countInput = page.getByTestId("sketch-pattern-count");
    await countInput.fill("3");
    await page.getByTestId("sketch-pattern-linear-x").click();
    await page.waitForTimeout(400);

    expect(await circleCount()).toBe(3);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeVisible();

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_circle").length).toBe(3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
