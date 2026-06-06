import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  settleSketchCamera,
  subCallTypes,
} from "./sketchHarness";

/**
 * Text as sketch geometry (F24). Text isn't a kernel feature tool — it's an
 * editing mode (like Trim / Offset): the user types a string + cap height,
 * picks the Text tool, then clicks to place SINGLE-STROKE (engraving-style)
 * text. Each glyph stroke is emitted as ordinary `sketch_line` sub-calls with
 * its lower-left corner at the click (see `web/src/sketchText.ts` +
 * store.placeSketchText). Filled outlines are a future enhancement.
 *
 * The layout/glyph code is exhaustively unit-tested; this E2E proves the
 * interactive path: set value "AB" + a height, pick Text, click → the session
 * and committed sketch each hold MANY sketch_line sub-calls, with no page
 * errors. Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch text", () => {
  test('placing "AB" emits many sketch_line strokes', async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    // Set the string + cap height.
    await page.getByTestId("sketch-text-value").fill("AB");
    await page.getByTestId("sketch-text-height").fill("20");

    // Pick the Text tool and click the canvas centre to place.
    await page.getByTestId("sketch-tool-sketch_text").click();
    await settleSketchCamera(page);
    const [cx, cy] = await canvasCentre(page);
    await page.mouse.click(cx, cy);
    await page.waitForTimeout(400);

    const lineCount = async () =>
      (await subCallTypes(page)).filter((t) => t === "sketch_line").length;

    expect(await lineCount()).toBeGreaterThan(4);

    // Finish the sketch; the committed feature carries the same lines.
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_line").length).toBeGreaterThan(
      4,
    );

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
