import { expect, test } from "@playwright/test";
import {
  sketchToClientPx,
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
    await page.getByTestId("sketch-tool-sketch_line").click();
    // Draw the line deliberately OFF-AXIS **in sketch space**. The old drag was
    // screen-diagonal (200 x 120 px) and looked off-axis — but the overlay
    // projects through the real plane frame, so that screen diagonal lands
    // near-horizontal in sketch coordinates and the line tool dutifully infers
    // a `sketch_constraint_horizontal`. This test wants a FREE line, so pick
    // the endpoints in sketch mm and project them.
    await settleSketchCamera(page);
    const [ax, ay] = await sketchToClientPx(page, -40, -20);
    const [bx, by] = await sketchToClientPx(page, 40, 20);
    await page.mouse.move(ax, ay);
    await page.mouse.down();
    await page.mouse.move((ax + bx) / 2, (ay + by) / 2, { steps: 5 });
    await page.mouse.move(bx, by, { steps: 10 });
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
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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
