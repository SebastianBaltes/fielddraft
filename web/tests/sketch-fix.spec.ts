import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  clickSketchPoint,
  committedSketchTypes,
  freshLoad,
  settleSketchCamera,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * Fix constraint via the palette. The solver, draft-state rebuild, palette
 * logic, and param mapping are all covered by Vitest / cargo; this E2E proves
 * the interactive path: draw a line, drop to Select mode, pick a single
 * endpoint, and confirm the palette surfaces the Fix button. Clicking it must
 * push a `sketch_constraint_fix` sub-call into the live session and survive a
 * re-solve. Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch fix constraint", () => {
  test("pick a single point, palette offers Fix, applying it persists", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    // Draw a line via raw pixel offsets from the canvas centre — independent
    // of the live camera, so it always lands on-canvas.
    const [cx, cy] = await canvasCentre(page);
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx - 100, cy - 60);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 5 });
    await page.mouse.move(cx + 100, cy + 60, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    expect(await subCallTypes(page)).toEqual(["sketch_line"]);

    // Pick the line's start endpoint at its ACTUAL committed coordinate.
    const start = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls[0]?.params?.start as [number, number] | undefined,
    );
    expect(start).toBeTruthy();

    await page.getByTestId("sketch-tool-select").click();
    const selection = await clickSketchPoint(page, start![0], start![1]);
    expect(selection.length).toBe(1);

    // The palette surfaces Fix for a single point.
    await expect(page.getByTestId("sketch-constraint-fix")).toBeVisible();
    await page.getByTestId("sketch-constraint-fix").click();
    await page.waitForTimeout(800); // let the solver land

    expect(await subCallTypes(page)).toContain("sketch_constraint_fix");

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_constraint_fix");

    // Re-solve survived: no uncaught page errors.
    await settleSketchCamera(page);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
