import { expect, test } from "@playwright/test";
import { freshLoad, subCallTypes, type SketchStoreWindow } from "./sketchHarness";

/**
 * Interactive line Extend (F12) — the complement of Trim. Extend isn't a
 * kernel tool; it's a single-click editing mode that lengthens the line
 * under the cursor along its own direction until it meets the nearest
 * other curve (see `web/src/sketchExtend.ts`). The geometry is
 * exhaustively unit-tested; this E2E proves the interactive path: draw a
 * short horizontal line and a separate vertical line to its right (not
 * touching), pick Extend, click the horizontal line near its right end,
 * and the line's right endpoint moves toward / onto the vertical line
 * (its x grows). Cold WebGPU first-paint is slow, so the timeout is
 * generous.
 */
test.describe.configure({ timeout: 120_000 });

/** Read the `end` coordinate of the i-th `sketch_line` sub-call. */
async function lineEnd(page: import("@playwright/test").Page, nth: number) {
  return page.evaluate((n) => {
    const calls =
      (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
        .subCalls ?? [];
    const lines = calls.filter((c) => c.type === "sketch_line");
    const p = lines[n]?.params?.end as [number, number] | undefined;
    return p ? [p[0], p[1]] : null;
  }, nth);
}

test.describe("Sketch extend", () => {
  test("extending a line lengthens its end toward the nearest curve", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    const drawLine = async (
      x1: number,
      y1: number,
      x2: number,
      y2: number,
    ) => {
      await page.getByTestId("sketch-tool-sketch_line").click();
      await page.mouse.move(x1, y1);
      await page.mouse.down();
      await page.mouse.move((x1 + x2) / 2, (y1 + y2) / 2, { steps: 4 });
      await page.mouse.move(x2, y2, { steps: 8 });
      await page.mouse.up();
      await page.waitForTimeout(150);
    };

    // A clearly horizontal segment pointing right, and a SEPARATE vertical
    // line further right (a gap between them — they do NOT touch).
    await drawLine(cx - 120, cy, cx - 10, cy);
    await drawLine(cx + 110, cy - 90, cx + 110, cy + 90);

    const types1 = await subCallTypes(page);
    expect(types1.filter((t) => t === "sketch_line").length).toBe(2);

    // The horizontal line is the first sketch_line; read its end before.
    const before = await lineEnd(page, 0);
    expect(before, "horizontal line end before extend").not.toBeNull();

    // Pick Extend and click the horizontal line near its RIGHT end.
    await page.getByTestId("sketch-tool-sketch_extend").click();
    await page.mouse.click(cx - 20, cy);
    await page.waitForTimeout(400);

    // Still two lines (extend rewrites in place, never adds geometry).
    const types2 = await subCallTypes(page);
    expect(types2.filter((t) => t === "sketch_line").length).toBe(2);

    const after = await lineEnd(page, 0);
    expect(after, "horizontal line end after extend").not.toBeNull();
    // The grabbed (right) end moved toward the vertical line: x increased.
    expect(
      after![0],
      `expected the right end x to grow (before ${JSON.stringify(
        before,
      )}, after ${JSON.stringify(after)})`,
    ).toBeGreaterThan(before![0] + 1);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeVisible();

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
