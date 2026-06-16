import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  subCallTypes,
} from "./sketchHarness";

/**
 * F6: regular N-gon tool. The gesture is a single drag from the
 * centre outward to set the radius; the side count + inscribed flag
 * come from the in-scene overlay shown while the tool is active
 * (ticket pqoe, plan item 1 — default 6, inscribed). Here we pick the
 * tool, set sides to 5 in the overlay, drag centre → radius via pixel
 * offsets from the canvas centre, and assert a single
 * `sketch_regular_polygon` sub-call with sides 5 lands — both live and
 * after Finish. Cold WebGPU first-paint can be slow, so the timeout is
 * generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch regular polygon tool", () => {
  test("centre → radius drag commits a sketch_regular_polygon", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);

    await page.getByTestId("enter-sketch").click();
    const toolButton = page.getByTestId(
      "sketch-tool-sketch_regular_polygon",
    );
    await expect(toolButton).toBeVisible();
    await toolButton.click();

    // Ticket pqoe (plan item 1): the side count + inscribed flag now live in an
    // in-scene overlay shown while the tool is active (no toolbar field). Set
    // the sides to 5 there so the committed sub-call carries the chosen value.
    const sidesInput = page.getByTestId("sketch-ngon-sides");
    await expect(sidesInput).toBeVisible();
    await sidesInput.fill("5");
    await expect(page.getByTestId("sketch-ngon-inscribed")).toBeVisible();

    const [cx, cy] = await canvasCentre(page);

    // Drag from the canvas centre outward to a point 80 px to the
    // right + 80 px up to set the radius.
    const radiusSx = cx + 80;
    const radiusSy = cy - 80;
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(radiusSx, radiusSy, { steps: 8 });
    await page.mouse.up();

    await page.waitForTimeout(300);

    // Live session holds exactly one regular-polygon sub-call.
    const live = await subCallTypes(page);
    expect(live).toContain("sketch_regular_polygon");

    // The committed-to-be sub-call carries the default side count.
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
    const ngon = subCalls.find((c) => c.type === "sketch_regular_polygon");
    expect(ngon).toBeTruthy();
    expect(Number(ngon!.params.sides)).toBe(5);

    await page.waitForTimeout(500);
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await committedSketchTypes(page);
    expect(committed).not.toBeNull();
    expect(committed).toContain("sketch_regular_polygon");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
