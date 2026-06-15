import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  clickSketchPoint,
  committedSketchTypes,
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * Point-on-object constraint via the palette. The solver, draft-state
 * rebuild, palette logic, and param mapping are all covered by Vitest /
 * cargo; this E2E proves the interactive path: draw a line and a circle,
 * drop to Select mode, pick the circle's centre point AND the line, and
 * confirm the palette surfaces the On-Curve button. Clicking it must push a
 * `sketch_constraint_point_on_object` sub-call into the live session and
 * survive a re-solve. Cold WebGPU first-paint is slow, so the timeout is
 * generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch point-on-object constraint", () => {
  test("pick a point + a line, palette offers On Curve, applying it persists", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    const [cx, cy] = await canvasCentre(page);

    // Draw a line via raw pixel offsets from the canvas centre — independent
    // of the live camera, so it always lands on-canvas.
    // Slanted (start/end at different y) so the auto-inference engine doesn't
    // silently add a horizontal constraint we'd have to account for.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx - 140, cy - 80);
    await page.mouse.down();
    await page.mouse.move(cx - 60, cy - 60, { steps: 5 });
    await page.mouse.move(cx + 20, cy - 40, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    // Draw a circle elsewhere (centre below-right of the line, dragged out
    // to a rim). Again purely pixel offsets.
    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.mouse.move(cx + 40, cy + 70);
    await page.mouse.down();
    await page.mouse.move(cx + 60, cy + 70, { steps: 5 });
    await page.mouse.move(cx + 100, cy + 70, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    const drawn = await subCallTypes(page);
    expect(drawn).toContain("sketch_line");
    expect(drawn).toContain("sketch_circle");

    // Read the committed line endpoints and circle centre from the live
    // session so the picks target their ACTUAL coordinates.
    const geom = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const line = calls.find((c) => c.type === "sketch_line");
      const circle = calls.find((c) => c.type === "sketch_circle");
      return {
        start: line?.params?.start as [number, number] | undefined,
        end: line?.params?.end as [number, number] | undefined,
        center: circle?.params?.center as [number, number] | undefined,
      };
    });
    expect(geom.start).toBeTruthy();
    expect(geom.end).toBeTruthy();
    expect(geom.center).toBeTruthy();

    await page.getByTestId("sketch-tool-select").click();

    // Pick the circle's centre point first (committed point pick).
    const sel1 = await clickSketchPoint(page, geom.center![0], geom.center![1]);
    expect(sel1.length).toBe(1);

    // Then add the line to the selection by clicking its midpoint. The pick
    // accumulates via toggleSketchSelection, so the selection grows to two.
    await settleSketchCamera(page);
    const midU = (geom.start![0] + geom.end![0]) / 2;
    const midV = (geom.start![1] + geom.end![1]) / 2;
    const [lpx, lpy] = await sketchToClientPx(page, midU, midV);
    await page.mouse.click(lpx, lpy);
    await page.waitForTimeout(200);

    const sel2 = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(sel2.length).toBe(2);

    // The palette surfaces On Curve for a point + a line.
    await expect(
      page.getByTestId("sketch-constraint-point_on_object"),
    ).toBeVisible();
    await page.getByTestId("sketch-constraint-point_on_object").click();
    await page.waitForTimeout(800); // let the solver land

    expect(await subCallTypes(page)).toContain(
      "sketch_constraint_point_on_object",
    );

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_constraint_point_on_object");

    // Re-solve survived: no uncaught page errors.
    await settleSketchCamera(page);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
