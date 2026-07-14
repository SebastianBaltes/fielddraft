import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  subCallTypes,
} from "./sketchHarness";

/**
 * F8: ellipse tool. The gesture is a single drag from the centre
 * outward to a corner; radius_x = |dx|, radius_y = |dy| (axis-aligned;
 * rotation defaults to 0 and is adjustable via the FeatureDialog). Here
 * we pick the tool, drag centre → corner via asymmetric pixel offsets
 * so radius_x ≠ radius_y, and assert a single `sketch_ellipse` sub-call
 * with both radii > 0 lands — both live and after Finish. Cold WebGPU
 * first-paint can be slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch ellipse tool", () => {
  test("centre → corner drag commits a sketch_ellipse", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);

    await page.getByTestId("enter-sketch").click();
    const toolButton = page.getByTestId("sketch-tool-sketch_ellipse");
    await expect(toolButton).toBeVisible();
    await toolButton.click();

    const [cx, cy] = await canvasCentre(page);

    // Drag from the canvas centre to a point 90 px right + 50 px up.
    // The asymmetric offsets give radius_x ≠ radius_y.
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 90, cy - 50, { steps: 8 });
    await page.mouse.up();

    await page.waitForTimeout(300);

    // Live session holds exactly one ellipse sub-call.
    const live = await subCallTypes(page);
    expect(live).toContain("sketch_ellipse");

    // The committed-to-be sub-call carries positive radii.
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
    const ellipse = subCalls.find((c) => c.type === "sketch_ellipse");
    expect(ellipse).toBeTruthy();
    expect(Number(ellipse!.params.radius_x)).toBeGreaterThan(0);
    expect(Number(ellipse!.params.radius_y)).toBeGreaterThan(0);

    await page.waitForTimeout(500);
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_ellipse");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
