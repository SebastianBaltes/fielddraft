import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  settleSketchCamera,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * Filled glyph text as sketch geometry (F24-full). Text isn't a kernel
 * feature tool — it's an editing mode (like Trim / Offset): the user types a
 * string + em height, picks a font, picks the Text tool, then clicks to
 * place. Each drawn glyph is emitted as ONE `sketch_path` sub-call whose
 * contours carry the glyph's outer ring plus any holes (the kernel fills it
 * with even-odd hole handling). Outlines come from a real font parsed with
 * opentype.js (see web/src/sketchText.ts + sketchTextFont.ts +
 * store.placeSketchText). Lowercase + punctuation lay out verbatim — no
 * uppercase folding.
 *
 * The layout/glyph code is exhaustively unit-tested; this E2E proves the
 * interactive path: set value "ABc" + a height, wait for the font to load,
 * pick Text, click → the session and committed sketch each hold one
 * `sketch_path` per drawn glyph, at least one with a HOLE (≥2 contours, from
 * A or B), the lowercase "c" also produces geometry, and no page errors.
 * Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

/** Count glyph paths + the max contour count across them, read live. */
async function pathStats(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const w = window as unknown as SketchStoreWindow;
    const calls = w.__fdStore?.state.sketchSession.subCalls ?? [];
    const paths = calls.filter((c) => c.type === "sketch_path");
    const maxContours = paths.reduce((m, p) => {
      const n =
        ((p.params as { contours?: unknown[] } | undefined)?.contours?.length) ??
        0;
      return Math.max(m, n);
    }, 0);
    return { pathCount: paths.length, maxContours };
  });
}

test.describe("Sketch text", () => {
  test('placing "ABc" emits one filled sketch_path per glyph, with a hole', async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    // Set the string + em height.
    await page.getByTestId("sketch-text-value").fill("ABc");
    await page.getByTestId("sketch-text-height").fill("20");

    // Pick the Text tool — this kicks off the async font load. Wait until the
    // store reports the font is parsed and ready before placing.
    await page.getByTestId("sketch-tool-sketch_text").click();
    await expect
      .poll(
        async () =>
          page.evaluate(
            () =>
              (window as unknown as SketchStoreWindow).__fdStore?.state
                .sketchTextFontReady === true,
          ),
        { timeout: 30_000 },
      )
      .toBe(true);

    await settleSketchCamera(page);
    const [cx, cy] = await canvasCentre(page);
    await page.mouse.click(cx, cy);
    await page.waitForTimeout(400);

    // One sketch_path per drawn glyph (A, B, c) and at least one glyph carries
    // a hole (≥2 contours — A and B both do).
    const stats = await pathStats(page);
    expect(stats.pathCount).toBeGreaterThanOrEqual(3);
    expect(stats.maxContours).toBeGreaterThanOrEqual(2);
    expect(await subCallTypes(page)).toContain("sketch_path");

    // Finish the sketch; the committed feature carries the same paths.
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(
      committed!.filter((t) => t === "sketch_path").length,
    ).toBeGreaterThanOrEqual(3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
