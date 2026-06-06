import { expect, test } from "@playwright/test";
import {
  committedSketchTypes,
  subCallTypes,
} from "./sketchHarness";

/**
 * Interactive line–line fillet (F10). Like Trim, fillet isn't a kernel
 * tool — it's a two-click editing mode that rounds the corner between
 * two picked lines with a tangent arc, trimming both lines back to the
 * tangent points (see `web/src/sketchFillet.ts`). The geometry is
 * exhaustively unit-tested; this E2E proves the interactive path: draw
 * two lines meeting at a corner, pick Fillet, click each line, and the
 * sketch gains a `sketch_arc` while keeping the two (now trimmed) lines.
 * Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch fillet", () => {
  test("filleting two lines rounds the corner with a tangent arc", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

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

    // Two lines sharing the corner at (cx, cy): one going right, one going
    // up. Both start at the corner so endpoint snapping keeps them
    // coincident — a sharp right-angle corner to round.
    await drawLine(cx, cy, cx + 140, cy);
    await drawLine(cx, cy, cx, cy - 140);

    const types1 = await subCallTypes(page);
    expect(types1.filter((t) => t === "sketch_line").length).toBe(2);

    // Pick the Fillet tool. The default radius (5 mm) is small relative to
    // the 140 px legs, so the tangent points stay on both segments.
    await page.getByTestId("sketch-tool-sketch_fillet").click();

    // Click near each line's MIDPOINT — curve picks are forgiving. First
    // the horizontal line, then the vertical line; the second click
    // applies the fillet.
    await page.mouse.click(cx + 70, cy);
    await page.waitForTimeout(150);
    await page.mouse.click(cx, cy - 70);
    await page.waitForTimeout(400);

    const types2 = await subCallTypes(page);
    expect(
      types2.filter((t) => t === "sketch_arc").length,
      `expected a sketch_arc after fillet, got ${JSON.stringify(types2)}`,
    ).toBe(1);
    expect(types2.filter((t) => t === "sketch_line").length).toBe(2);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_arc").length).toBe(1);
    expect(committed!.filter((t) => t === "sketch_line").length).toBe(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
