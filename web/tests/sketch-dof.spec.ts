import { expect, test } from "@playwright/test";
import { canvasCentre, freshLoad, subCallTypes } from "./sketchHarness";

/**
 * Degrees-of-freedom HUD (F5). The DoF math itself is unit-tested in
 * dofPreflight.test.ts; this E2E proves the live readout: it appears in
 * sketch mode, reports the fully-constrained empty sketch, and climbs as
 * geometry with free DoF is drawn.
 *
 * An empty sketch is just the fixed origin (0 DoF), so countDegreesOfFreedom
 * returns 0 → the readout reads "Fully constrained". Drawing a free line adds
 * two free endpoints (4 DoF) → a positive "N DoF" count.
 *
 * Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch DoF readout", () => {
  test("shows fully-constrained for an empty sketch, then climbs as a line is drawn", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    // Empty sketch = origin only = 0 DoF → "Fully constrained".
    const dofReadout = page.getByTestId("sketch-dof");
    await expect(dofReadout).toBeVisible();
    await expect(dofReadout).toHaveText("Fully constrained");

    // Draw a line via raw pixel offsets from the canvas centre — camera
    // independent, so it always lands on-canvas.
    const [cx, cy] = await canvasCentre(page);
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx - 100, cy - 60);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 5 });
    await page.mouse.move(cx + 100, cy + 60, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    expect(await subCallTypes(page)).toEqual(["sketch_line"]);

    // The line's two free endpoints add DoF → a positive "N DoF" count.
    await expect(dofReadout).toHaveText(/^\d+ DoF$/);
    const text = await dofReadout.textContent();
    const dof = Number.parseInt(text!.replace(/\D/g, ""), 10);
    expect(dof).toBeGreaterThan(0);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
