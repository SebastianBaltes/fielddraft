import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  committedSketchTypes,
  freshLoad,
  subCallTypes,
} from "./sketchHarness";

/**
 * F7: primitive-construction variants — rectangle (center),
 * circle (3-point), rectangle (3-point).
 *
 * These are UI-only tools (like trim): they emit ordinary sub-calls at
 * commit time. We assert the live session sub-calls right after the gesture,
 * then Finish and assert the committed sketch carries them.
 *
 *   - rectangle (center): drag centre → corner → one `sketch_rectangle`.
 *   - circle (3-point): three clicks → one `sketch_circle`.
 *   - rectangle (3-point): three clicks → four `sketch_line` + perpendicular
 *     constraints.
 *
 * Draw gestures use raw pixel offsets from the canvas centre, which is
 * camera-independent and always on-canvas. Cold WebGPU first-paint can be
 * slow so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

const count = (arr: string[], t: string) => arr.filter((x) => x === t).length;

test.describe("Sketch primitive variants", () => {
  test("rectangle (center) drag commits a sketch_rectangle", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    await page.getByTestId("sketch-tool-sketch_rectangle_center").click();

    const [cx, cy] = await canvasCentre(page);
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 40, cy - 25, { steps: 4 });
    await page.mouse.move(cx + 80, cy - 50, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(400);

    expect(count(await subCallTypes(page), "sketch_rectangle")).toBe(1);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    expect(count((await committedSketchTypes(page)) ?? [], "sketch_rectangle")).toBe(1);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("circle (3-point) three clicks commit a sketch_circle", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    await page.getByTestId("sketch-tool-sketch_circle_3p").click();

    const [cx, cy] = await canvasCentre(page);
    // Three non-collinear points: right, top, left.
    await page.mouse.click(cx + 70, cy);
    await page.mouse.move(cx, cy - 70, { steps: 4 });
    await page.mouse.click(cx, cy - 70);
    await page.mouse.move(cx - 70, cy, { steps: 4 });
    await page.mouse.click(cx - 70, cy);
    await page.waitForTimeout(400);

    expect(count(await subCallTypes(page), "sketch_circle")).toBe(1);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    expect(count((await committedSketchTypes(page)) ?? [], "sketch_circle")).toBe(1);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("rectangle (3-point) three clicks commit four sketch_line", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    await page.getByTestId("sketch-tool-sketch_rectangle_3p").click();

    const [cx, cy] = await canvasCentre(page);
    // p1 → p2 sets the base edge (slanted to avoid axis auto-constraints),
    // p3 sets the perpendicular width.
    await page.mouse.click(cx - 90, cy + 40);
    await page.mouse.move(cx + 60, cy - 10, { steps: 4 });
    await page.mouse.click(cx + 60, cy - 10);
    await page.mouse.move(cx + 30, cy - 80, { steps: 4 });
    await page.mouse.click(cx + 30, cy - 80);
    await page.waitForTimeout(400);

    expect(count(await subCallTypes(page), "sketch_line")).toBe(4);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    expect(count((await committedSketchTypes(page)) ?? [], "sketch_line")).toBe(4);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
