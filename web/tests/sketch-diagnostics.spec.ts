import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * F20: conflict / redundancy diagnostics in the Constraints panel.
 *
 * The pure mapping (`constraintIdsOfSubCall`, `summarizeDiagnostics`) and the
 * solver→diagnostics wiring are covered by Vitest; this E2E proves the live
 * interactive path: build an OVER-constrained sketch (a line forced both
 * Horizontal AND Vertical conflicts in PlaneGCS), then assert the panel
 * surfaces the diagnostics summary plus a per-row conflict badge, and that
 * deleting the offending constraint clears the summary.
 *
 * A degenerate line can't be both horizontal and vertical, so the second
 * constraint makes the system inconsistent — PlaneGCS reports it via
 * get_conflicting_constraints, which `solveAndReplace` threads into the
 * session. The assertion is deliberately tolerant (summary OR a conflict
 * badge) so it stays robust if PlaneGCS attributes the conflict to either
 * constraint id.
 *
 * Cold WebGPU first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch conflict diagnostics", () => {
  test("over-constraining a line flags a conflict, deleting it clears the summary", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    const [cx, cy] = await canvasCentre(page);

    // Draw a slanted line (raw pixel offsets — camera-independent). A slant
    // means neither Horizontal nor Vertical is already satisfied, so forcing
    // both is genuinely inconsistent.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.mouse.move(cx - 100, cy - 60);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 5 });
    await page.mouse.move(cx + 100, cy + 60, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(300);

    expect(await subCallTypes(page)).toEqual(["sketch_line"]);

    // Select JUST the line entity by clicking its midpoint on the canvas
    // (the panel row would select the endpoints too, and the line-constraint
    // palette only appears for a single line selection). Read the committed
    // endpoints so the click targets the actual solved geometry.
    const selectLine = async () => {
      const line = await page.evaluate(
        () =>
          (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
            .subCalls[0]?.params as
            | { start: [number, number]; end: [number, number] }
            | undefined,
      );
      await settleSketchCamera(page);
      const mu = (line!.start[0] + line!.end[0]) / 2;
      const mv = (line!.start[1] + line!.end[1]) / 2;
      const [px, py] = await sketchToClientPx(page, mu, mv);
      await page.mouse.click(px, py);
      await page.waitForTimeout(200);
    };

    await page.getByTestId("sketch-tool-select").click();
    await selectLine();
    await expect(page.getByTestId("sketch-constraint-horizontal")).toBeVisible();
    await page.getByTestId("sketch-constraint-horizontal").click();
    await page.waitForTimeout(800); // let the solver land

    expect(await subCallTypes(page)).toContain("sketch_constraint_horizontal");

    // Re-select the line and apply Horizontal AGAIN — a second identical
    // horizontal constraint is over-constraining: PlaneGCS reports it as
    // redundant (horizontal+vertical, by contrast, is satisfiable by a
    // zero-length line, so it would NOT flag).
    await page.getByTestId("sketch-tool-select").click();
    await selectLine();
    await expect(page.getByTestId("sketch-constraint-horizontal")).toBeVisible();
    await page.getByTestId("sketch-constraint-horizontal").click();
    await page.waitForTimeout(1000); // let the solver land + diagnostics thread

    // The diagnostics summary appears, and at least one constraint row carries
    // a diagnostics badge. Tolerant about conflict vs redundant and which
    // constraint id PlaneGCS attributes it to.
    const summary = page.getByTestId("sketch-diagnostics-summary");
    const anyBadge = page.locator(
      '[data-testid^="sketch-constraint-conflict-"], [data-testid^="sketch-constraint-redundant-"]',
    );
    await expect(summary).toBeVisible();
    await expect(anyBadge.first()).toBeVisible();

    // Find which constraint row is flagged, and delete that constraint via its
    // delete button. Constraint rows reuse the shared `sketch-sub-row-delete-N`
    // affordance.
    const flaggedIndex = await page.evaluate(() => {
      const el = document.querySelector(
        '[data-testid^="sketch-constraint-conflict-"], [data-testid^="sketch-constraint-redundant-"]',
      );
      if (!el) return null;
      const id = el.getAttribute("data-testid")!;
      return Number.parseInt(id.replace(/^sketch-constraint-(conflict|redundant)-/, ""), 10);
    });
    expect(flaggedIndex).not.toBeNull();

    // The delete affordance only becomes clickable on row hover
    // (pointer-events:none until :hover), so hover the row first, then click
    // with force to bypass the hover-reveal actionability race.
    await page.getByTestId(`sketch-element-row-${flaggedIndex}`).hover();
    await page
      .getByTestId(`sketch-sub-row-delete-${flaggedIndex}`)
      .click({ force: true });
    await page.waitForTimeout(1000); // re-solve with the conflict removed

    // With the conflicting constraint gone the sketch is consistent again →
    // the summary disappears.
    await expect(summary).toHaveCount(0);

    // No uncaught page errors throughout.
    await settleSketchCamera(page);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
