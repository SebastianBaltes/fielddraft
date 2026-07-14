import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  sketchToClientPx,
  settleSketchCamera,
  subCallTypes,
} from "./sketchHarness";

/**
 * F23-full: control-point cubic B-spline.
 *
 * Gesture under test:
 *   • Pick the spline tool, then switch the mode toggle to "Control pts"
 *     (a real degree-3 B-spline whose clicked points are the control
 *     polygon).
 *   • Single-click drops a control point.
 *   • DOUBLE-CLICK finishes the spline as an OPEN curve.
 *
 * Four control points → one open `sketch_spline` sub-call with
 * `mode === "control"` and 8 numbers in `params.points`. Then we drag
 * the first control-point grip and assert the control point moved (the
 * curve re-evaluates via de Boor live). Finally Finish commits.
 *
 * Cold WebGPU first-paint can be slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch control-point B-spline tool", () => {
  test("control-point B-spline commits, renders, and grips drag", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);

    await page.getByTestId("enter-sketch").click();
    const toolButton = page.getByTestId("sketch-tool-sketch_spline");
    await expect(toolButton).toBeVisible();
    await toolButton.click();

    // Switch to control-point (B-spline) mode.
    const modeControl = page.getByTestId("sketch-spline-mode-control");
    await expect(modeControl).toBeVisible();
    await modeControl.click();
    await expect(modeControl).toHaveAttribute("aria-pressed", "true");

    const [cx, cy] = await canvasCentre(page);

    // Four control points as pixel offsets from the canvas centre.
    const pts: Array<[number, number]> = [
      [cx - 90, cy],
      [cx - 30, cy - 70],
      [cx + 30, cy + 70],
      [cx + 90, cy],
    ];

    for (let i = 0; i < 3; i++) {
      await page.mouse.click(pts[i][0], pts[i][1]);
      await page.waitForTimeout(120);
    }
    await page.mouse.dblclick(pts[3][0], pts[3][1]);
    await page.waitForTimeout(300);

    // Live session holds a control-mode spline sub-call with 4 control
    // points (8 numbers).
    const live = await subCallTypes(page);
    expect(live).toContain("sketch_spline");

    const readSpline = () =>
      page.evaluate(() => {
        const calls =
          (
            window as unknown as {
              __fdStore?: {
                state: {
                  sketchSession: {
                    subCalls: Array<{
                      type: string;
                      params: Record<string, unknown>;
                    }>;
                  };
                };
              };
            }
          ).__fdStore?.state.sketchSession.subCalls ?? [];
        const s = calls.find((c) => c.type === "sketch_spline");
        return s
          ? { mode: s.params.mode, points: s.params.points as number[] }
          : null;
      });

    const before = await readSpline();
    expect(before).toBeTruthy();
    expect(before!.mode).toBe("control");
    expect(Array.isArray(before!.points)).toBe(true);
    expect(before!.points.length).toBe(8);

    // Grips live in SELECT mode for the current selection. Switch to
    // select, click the first control point to select it (so its grip
    // appears), then drag that grip. The curve re-evaluates (de Boor)
    // live and re-solves on drop.
    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);
    const cp0u = before!.points[0];
    const cp0v = before!.points[1];
    let [gx, gy] = await sketchToClientPx(page, cp0u, cp0v);
    await page.mouse.click(gx, gy);
    await page.waitForTimeout(200);
    // Re-project after any auto-fit nudge, then drag the grip.
    await settleSketchCamera(page);
    [gx, gy] = await sketchToClientPx(page, cp0u, cp0v);
    await page.mouse.move(gx, gy);
    await page.mouse.down();
    await page.mouse.move(gx - 45, gy - 45, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    const after = await readSpline();
    expect(after).toBeTruthy();
    // The first control point moved (grip drag updated the right point).
    const moved =
      Math.abs(after!.points[0] - before!.points[0]) > 1 ||
      Math.abs(after!.points[1] - before!.points[1]) > 1;
    expect(moved, "first control point did not move after grip drag").toBe(true);
    // It is still a control-mode spline with 4 control points.
    expect(after!.mode).toBe("control");
    expect(after!.points.length).toBe(8);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_spline");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
