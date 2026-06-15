import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  subCallTypes,
} from "./sketchHarness";

/**
 * F9: centre-to-centre slot (stadium) tool. The gesture is a single
 * drag from end-centre A to end-centre B; the radius comes from the
 * FeatureDialog (default 8 mm). Here we pick the tool, drag A→B via
 * horizontal pixel offsets (cx−80,cy → cx+80,cy), and assert a single
 * `sketch_slot` sub-call carrying center_a/center_b/radius lands — both
 * live and after Finish. Cold WebGPU first-paint can be slow, so the
 * timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch slot tool", () => {
  test("A → B drag commits a sketch_slot", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);

    await page.getByTestId("enter-sketch").click();
    const toolButton = page.getByTestId("sketch-tool-sketch_slot");
    await expect(toolButton).toBeVisible();
    await toolButton.click();

    const [cx, cy] = await canvasCentre(page);

    // Drag horizontally: end-centre A at cx−80, end-centre B at cx+80.
    await page.mouse.move(cx - 80, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 80, cy, { steps: 8 });
    await page.mouse.up();

    await page.waitForTimeout(300);

    // Live session holds a slot sub-call.
    const live = await subCallTypes(page);
    expect(live).toContain("sketch_slot");

    // The committed-to-be sub-call carries both centres and a radius.
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
    const slot = subCalls.find((c) => c.type === "sketch_slot");
    expect(slot).toBeTruthy();
    expect(Array.isArray(slot!.params.center_a)).toBe(true);
    expect(Array.isArray(slot!.params.center_b)).toBe(true);
    expect(Number(slot!.params.radius)).toBeGreaterThan(0);

    await page.waitForTimeout(500);
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_slot");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
