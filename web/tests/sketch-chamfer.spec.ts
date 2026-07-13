import { expect, test } from "@playwright/test";
import {
  committedSketchTypes,
  subCallTypes,
} from "./sketchHarness";

/**
 * Interactive line–line chamfer (F11). The straight-edge sibling of
 * Fillet: a two-click editing mode that bevels the corner between two
 * picked lines with a straight connecting line, trimming both lines back
 * to their setback points (see `web/src/sketchChamfer.ts`). The geometry
 * is exhaustively unit-tested; this E2E proves the interactive path: draw
 * two lines meeting at a corner, pick Chamfer, click each line, and the
 * sketch keeps the two (now trimmed) lines and gains a third straight
 * connector line — no arc. Cold WebGPU first-paint is slow, so the
 * timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch chamfer", () => {
  test("chamfering two lines bevels the corner with a straight line", async ({
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
    // coincident — a sharp right-angle corner to bevel.
    await drawLine(cx, cy, cx + 140, cy);
    await drawLine(cx, cy, cx, cy - 140);

    const types1 = await subCallTypes(page);
    expect(types1.filter((t) => t === "sketch_line").length).toBe(2);

    // Pick the Chamfer tool. The default distance (5 mm) is small relative
    // to the 140 px legs, so both setback points stay on their segments.
    await page.getByTestId("sketch-tool-sketch_chamfer").click();

    // Click near each line's MIDPOINT — curve picks are forgiving. First
    // the horizontal line, then the vertical line; the second click
    // applies the chamfer.
    await page.mouse.click(cx + 70, cy);
    await page.waitForTimeout(150);
    await page.mouse.click(cx, cy - 70);
    await page.waitForTimeout(400);

    const types2 = await subCallTypes(page);
    expect(
      types2.filter((t) => t === "sketch_arc").length,
      `expected NO sketch_arc after chamfer, got ${JSON.stringify(types2)}`,
    ).toBe(0);
    expect(
      types2.filter((t) => t === "sketch_line").length,
      `expected 3 sketch_line (2 trimmed + 1 connector), got ${JSON.stringify(types2)}`,
    ).toBe(3);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeVisible();

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_arc").length).toBe(0);
    expect(committed!.filter((t) => t === "sketch_line").length).toBe(3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
