import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  subCallTypes,
} from "./sketchHarness";

/**
 * Interactive split (F13). Split isn't a kernel tool — it's the keep-both
 * sibling of Trim: a click cuts the curve under the cursor into two pieces
 * that meet at the click point; nothing is deleted (see
 * `web/src/sketchSplit.ts`). The split geometry is exhaustively unit-tested;
 * this E2E proves the interactive path: draw ONE line, click its midpoint in
 * Split mode, and the single line becomes two — in the live session and in
 * the committed sketch. Cold WebGPU first-paint is slow, so the timeout is
 * generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch split", () => {
  test("clicking a line at its midpoint splits it into two", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    const [cx, cy] = await canvasCentre(page);

    // Draw one horizontal line across the canvas centre (raw pixel offsets —
    // camera-independent, always on-canvas).
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx - 120, cy);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 4 });
    await page.mouse.move(cx + 120, cy, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    expect(
      (await subCallTypes(page)).filter((t) => t === "sketch_line").length,
    ).toBe(1);

    // Split: pick the mode, click the line near its MIDPOINT (canvas centre).
    await page.getByTestId("sketch-tool-sketch_split").click();
    await page.mouse.click(cx, cy);
    await page.waitForTimeout(400);

    expect(
      (await subCallTypes(page)).filter((t) => t === "sketch_line").length,
    ).toBe(2);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed!.filter((t) => t === "sketch_line").length).toBe(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
