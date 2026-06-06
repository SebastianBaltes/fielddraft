import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  subCallTypes,
} from "./sketchHarness";

/**
 * F23: interpolating spline tool (Catmull-Rom through clicked control
 * points, lowered kernel-side to a quad-Bézier Path).
 *
 * Gesture under test (multi-click, mirrors the polygon tool):
 *   • Single-click drops a control point.
 *   • DOUBLE-CLICK finishes the spline as an OPEN curve (the first click
 *     of the double-click places the final control point; the second
 *     click, with `detail >= 2`, commits).
 *   • (Not exercised here: clicking within 8 px of the FIRST point with
 *     ≥ 3 points closes the loop; Enter finishes open, Escape cancels.)
 *
 * So the four control points below are placed as three single clicks
 * plus one double-click at the fourth location → an open `sketch_spline`
 * with 4 control points (8 numbers in `params.points`).
 *
 * Cold WebGPU first-paint can be slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch spline tool", () => {
  test("four clicked points commit an open sketch_spline", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);

    await page.getByTestId("enter-sketch").click();
    const toolButton = page.getByTestId("sketch-tool-sketch_spline");
    await expect(toolButton).toBeVisible();
    await toolButton.click();

    const [cx, cy] = await canvasCentre(page);

    // Four control points as pixel offsets from the canvas centre.
    const pts: Array<[number, number]> = [
      [cx - 90, cy],
      [cx - 30, cy - 60],
      [cx + 30, cy + 60],
      [cx + 90, cy],
    ];

    // First three points: single clicks.
    for (let i = 0; i < 3; i++) {
      await page.mouse.click(pts[i][0], pts[i][1]);
      await page.waitForTimeout(120);
    }
    // Fourth point + finish: a double-click drops the last point and
    // commits the open spline.
    await page.mouse.dblclick(pts[3][0], pts[3][1]);
    await page.waitForTimeout(300);

    // Live session holds a spline sub-call with 4 control points.
    const live = await subCallTypes(page);
    expect(live).toContain("sketch_spline");

    const subCalls = await page.evaluate(
      () =>
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
        ).__fdStore?.state.sketchSession.subCalls ?? [],
    );
    const spline = subCalls.find((c) => c.type === "sketch_spline");
    expect(spline).toBeTruthy();
    expect(Array.isArray(spline!.params.points)).toBe(true);
    expect((spline!.params.points as number[]).length).toBe(8);

    await page.waitForTimeout(500);
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_spline");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
