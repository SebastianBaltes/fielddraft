import { expect, test } from "@playwright/test";
import {
  canvasCentre,
  freshLoad,
  sketchToClientPx,
  settleSketchCamera,
  subCallTypes,
} from "./sketchHarness";

/**
 * F23 endpoint tangent (G1): the end "leg" of an open control-mode
 * B-spline is a real `line` entity (cp[N-2] → cp[N-1]). Selecting that
 * leg together with an adjacent line and applying the ordinary Parallel
 * constraint makes the spline END tangent to the line — no new
 * constraint type, the existing line palette does it.
 *
 * Gesture under test:
 *   • Spline tool → Control-pts mode → drop 4 control points, finish
 *     OPEN with a double-click.
 *   • Line tool → draw an adjacent line at a deliberately non-parallel
 *     angle to the spline's end leg.
 *   • Select mode → click the spline END leg, shift-click the line.
 *   • Apply Parallel from the constraint palette.
 *   • Read back the solved control points and assert the end-leg
 *     direction (cp2 → cp3) is parallel to the line direction.
 *
 * Cold WebGPU first-paint can be slow, so the timeout is generous. The
 * reviewer runs this on real WebGPU during abnahme.
 */
test.describe.configure({ timeout: 120_000 });

interface SplineReadWindow {
  __fdStore?: {
    state: {
      sketchSession: {
        selection: number[];
        subCalls: Array<{ type: string; params: Record<string, unknown> }>;
      };
    };
  };
}

/** Cross-product magnitude of two 2D vectors; 0 ⇒ parallel. */
function cross(ax: number, ay: number, bx: number, by: number): number {
  return ax * by - ay * bx;
}

test.describe("Sketch B-spline endpoint tangent (Parallel on end leg)", () => {
  test("end leg becomes parallel to an adjacent line after solve", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();

    // --- Draw an open control-mode B-spline (4 control points) -------
    const splineTool = page.getByTestId("sketch-tool-sketch_spline");
    await expect(splineTool).toBeVisible();
    await splineTool.click();
    const modeControl = page.getByTestId("sketch-spline-mode-control");
    await expect(modeControl).toBeVisible();
    await modeControl.click();
    await expect(modeControl).toHaveAttribute("aria-pressed", "true");

    const [cx, cy] = await canvasCentre(page);
    const cps: Array<[number, number]> = [
      [cx - 120, cy + 40],
      [cx - 40, cy - 60],
      [cx + 40, cy - 20],
      [cx + 120, cy + 60], // end leg cp2→cp3 is clearly slanted
    ];
    for (let i = 0; i < 3; i++) {
      await page.mouse.click(cps[i][0], cps[i][1]);
      await page.waitForTimeout(120);
    }
    await page.mouse.dblclick(cps[3][0], cps[3][1]);
    await page.waitForTimeout(300);

    // --- Draw an adjacent line at a deliberately different angle -----
    const lineTool = page.getByTestId("sketch-tool-sketch_line");
    await expect(lineTool).toBeVisible();
    await lineTool.click();
    // A near-horizontal line, far below the spline so it doesn't snap.
    await page.mouse.click(cx - 100, cy + 160);
    await page.waitForTimeout(120);
    await page.mouse.dblclick(cx + 100, cy + 150);
    await page.waitForTimeout(300);

    const live = await subCallTypes(page);
    expect(live).toContain("sketch_spline");
    expect(live).toContain("sketch_line");

    const readState = () =>
      page.evaluate(() => {
        const calls =
          (window as unknown as SplineReadWindow).__fdStore?.state.sketchSession
            .subCalls ?? [];
        const spline = calls.find((c) => c.type === "sketch_spline");
        const lineCall = calls.find((c) => c.type === "sketch_line");
        return {
          points: (spline?.params.points as number[]) ?? null,
          lineStart: (lineCall?.params.start as number[]) ?? null,
          lineEnd: (lineCall?.params.end as number[]) ?? null,
        };
      });

    const before = await readState();
    expect(before.points, "spline points missing").not.toBeNull();
    expect(before.points!.length).toBe(8);

    // --- Select the spline END leg + the line, apply Parallel --------
    await page.getByTestId("sketch-tool-select").click();
    await settleSketchCamera(page);

    // End leg is cp[2] → cp[3]; click its MIDPOINT (a line pick, so a
    // small projection error still lands on the segment).
    const p = before.points!;
    const legMidU = (p[4] + p[6]) / 2;
    const legMidV = (p[5] + p[7]) / 2;
    let [lx, ly] = await sketchToClientPx(page, legMidU, legMidV);
    await page.mouse.click(lx, ly);
    await page.waitForTimeout(200);

    // Add the line to the selection (shift-click its midpoint).
    await settleSketchCamera(page);
    const lineMidU = (before.lineStart![0] + before.lineEnd![0]) / 2;
    const lineMidV = (before.lineStart![1] + before.lineEnd![1]) / 2;
    const [mx, my] = await sketchToClientPx(page, lineMidU, lineMidV);
    await page.keyboard.down("Shift");
    await page.mouse.click(mx, my);
    await page.keyboard.up("Shift");
    await page.waitForTimeout(200);

    const selection = await page.evaluate(
      () =>
        (window as unknown as SplineReadWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(selection.length, "expected the leg + the line selected").toBe(2);

    const parallelBtn = page.getByTestId("sketch-constraint-parallel");
    await expect(parallelBtn).toBeVisible();
    await parallelBtn.click();
    await page.waitForTimeout(600);

    // --- Assert the end leg is now parallel to the line --------------
    const after = await readState();
    expect(after.points, "spline points missing after solve").not.toBeNull();
    const a = after.points!;
    const legDx = a[6] - a[4];
    const legDy = a[7] - a[5];
    const lineDx = after.lineEnd![0] - after.lineStart![0];
    const lineDy = after.lineEnd![1] - after.lineStart![1];

    const c = cross(legDx, legDy, lineDx, lineDy);
    const legLen = Math.hypot(legDx, legDy);
    const lineLen = Math.hypot(lineDx, lineDy);
    // Normalised cross-product (= sin of the angle between them) ≈ 0.
    const sinAngle = Math.abs(c) / Math.max(legLen * lineLen, 1e-9);
    expect(
      sinAngle,
      `end leg not parallel to line (sin=${sinAngle})`,
    ).toBeLessThan(0.02);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
