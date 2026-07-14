import { expect, test } from "@playwright/test";
import {
  pickSketchTool,
  canvasCentre,
  expandGeometrySection,
  freshLoad,
  settleSketchCamera,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * F19: Elements / Constraints panel. The pure row→entity-ids and
 * row→constraint-referents mappings are covered by Vitest; this E2E proves the
 * interactive path: the sub-spine splits into a "Geometry" section and a
 * "Constraints" section with live counts, each row is click-to-select, and the
 * delete affordance still drops the sub-call without also triggering a select.
 *
 * Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch elements panel", () => {
  test("rows are click-to-select and sections show live counts", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    // The elements panel (Geometry/Constraints sections + rows + delete) lives
    // inside the collapsed-by-default "Geometry & Constraints" sub-accordion
    // (ticket 87a1).
    await expandGeometrySection(page);

    const [cx, cy] = await canvasCentre(page);

    // Draw a line (raw pixel offsets — camera-independent, always on-canvas).
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx - 120, cy - 60);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 5 });
    await page.mouse.move(cx + 120, cy + 40, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    // Draw a circle.
    await pickSketchTool(page, "sketch_circle");
    await page.mouse.move(cx - 80, cy + 80);
    await page.mouse.down();
    await page.mouse.move(cx - 40, cy + 80, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    // The line drag is near-horizontal IN SKETCH SPACE (the overlay projects
    // through the real plane frame, so a screen-diagonal drag is not a
    // sketch-diagonal one), so the line tool infers a horizontal constraint
    // from the end snap and it lands between the two geometry calls.
    const types = await subCallTypes(page);
    expect(types).toEqual([
      "sketch_line",
      "sketch_constraint_horizontal",
      "sketch_circle",
    ]);

    // The Geometry section header is present with the right count.
    const geomHeader = page.getByTestId("sketch-elements-geometry");
    await expect(geomHeader).toBeVisible();
    await expect(geomHeader).toHaveText(/Geometry \(2\)/);

    // Find the row index of the circle (it's the 2nd sub-call → index 1).
    const circleIndex = types.indexOf("sketch_circle");
    expect(circleIndex).toBeGreaterThanOrEqual(0);

    // Click the circle's geometry row → its entity ids become the selection.
    await page.getByTestId(`sketch-element-row-${circleIndex}`).click();
    await page.waitForTimeout(200);

    const selection = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(selection.length).toBeGreaterThan(0);

    // The clicked row reads as active.
    await expect(
      page.getByTestId(`sketch-element-row-${circleIndex}`),
    ).toHaveAttribute("aria-pressed", "true");

    // Add a constraint: select the line (index 0), apply Horizontal.
    await page.getByTestId(`sketch-element-row-0`).click();
    await page.waitForTimeout(200);
    const horizontalBtn = page.getByTestId("sketch-constraint-horizontal");
    if (await horizontalBtn.count()) {
      await horizontalBtn.click();
      await page.waitForTimeout(600); // let the solver land

      const withConstraint = await subCallTypes(page);
      expect(withConstraint).toContain("sketch_constraint_horizontal");

      // The Constraints section appears with a count.
      const consHeader = page.getByTestId("sketch-elements-constraints");
      await expect(consHeader).toBeVisible();
      await expect(consHeader).toHaveText(/Constraints \(\d+\)/);

      // Clicking the constraint row selects its referents (the line).
      const consIndex = withConstraint.indexOf("sketch_constraint_horizontal");
      await page.getByTestId(`sketch-element-row-${consIndex}`).click();
      await page.waitForTimeout(200);
      const consSelection = await page.evaluate(
        () =>
          (window as unknown as SketchStoreWindow).__fdStore?.state
            .sketchSession.selection ?? [],
      );
      expect(consSelection.length).toBeGreaterThan(0);
    }

    // Delete a geometry row via its delete button. The count drops by MORE than
    // one: removing a line CASCADES to the constraints that referenced it (here
    // its horizontal), so `before - 1` was never the right arithmetic. State the
    // actual contract instead — the line and its constraint are gone, the circle
    // survives, and the click does NOT leak into a row-select (stopPropagation).
    const before = await subCallTypes(page);
    await page.getByTestId(`sketch-sub-row-delete-0`).click();
    await page.waitForTimeout(300);
    const after = await subCallTypes(page);
    expect(after.length).toBeLessThan(before.length);
    expect(after).not.toContain("sketch_line");
    expect(after).not.toContain("sketch_constraint_horizontal");
    expect(after).toContain("sketch_circle");

    // No uncaught page errors.
    await settleSketchCamera(page);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
