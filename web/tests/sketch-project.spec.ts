import { expect, test } from "@playwright/test";
import {
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
} from "./sketchHarness";

/**
 * F21: Project / Use external geometry.
 *
 * Existing bodies' analytic B-Rep edges are projected onto the active sketch
 * plane and drawn as faint reference guides. The Project tool adopts the
 * guide under the cursor as a real CONSTRUCTION `sketch_line` the solver can
 * reference (see `web/src/sketchProject.ts`). The nearest-edge pick is
 * exhaustively unit-tested; this E2E proves the interactive path end-to-end
 * on real WebGPU: a guide is present, a click adopts it, and the resulting
 * sub-call is a construction line.
 *
 * HOW WE OBTAIN PROJECTED EDGES
 * -----------------------------
 * A default box is added via the dev store hook `window.__fdStore.addPrimitive
 * ("box")` (half-extents [50,50,50], centred at the origin → footprint spans
 * u,v ∈ [-50, 50] on the XY plane). With a body present, the toolbar's
 * `enter-sketch` button would trigger FACE-PICK mode, which requires clicking
 * a face in the 3D WebGPU viewport — unreliable headless. So we enter the
 * sketch directly on the XY origin plane via the dev hook
 * `window.__fdStore.enterSketch("qCreatedBy(origin_plane_xy)")`. The box's
 * edges then project onto that plane as guides, and we click near the known
 * footprint edge midpoint (0, +50) to adopt it.
 *
 * This spec DOES assert that a projected guide was truly adopted: after the
 * click, `subCallTypes(page)` must contain a `sketch_line` whose
 * params.construction is true (the only way such a line can appear here is
 * via the Project tool — no line was drawn manually).
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch project (use external geometry)", () => {
  test("clicking a projected box edge adopts it as a construction line", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);

    // 1) Add a default box (origin-centred, half-extents 50) via the dev
    //    store hook, then enter a sketch on the XY origin plane directly —
    //    bypassing the 3D face-pick that a body would otherwise force.
    const entered = await page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            addPrimitive: (k: string) => number;
            enterSketch: (q: string) => void;
          };
        }
      ).__fdStore;
      if (!store) return false;
      store.addPrimitive("box");
      store.enterSketch("qCreatedBy(origin_plane_xy)");
      return true;
    });
    expect(entered, "dev store hook __fdStore unavailable").toBe(true);

    // The 2D overlay should now be live.
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    await settleSketchCamera(page);

    // 2) Pick the Project tool.
    await page.getByTestId("sketch-tool-sketch_project").click();
    await expect(page.getByTestId("sketch-project-hint")).toBeVisible();

    // 3) Click near the +Y footprint edge midpoint. The box spans
    //    u,v ∈ [-50, 50] on the XY plane, so its top edge runs from
    //    (-50, 50) to (50, 50); its midpoint is (0, 50). Project the
    //    sketch-mm point to a live client pixel and click it.
    const [px, py] = await sketchToClientPx(page, 0, 50);
    await page.mouse.click(px, py);
    await page.waitForTimeout(300);

    // 4) A construction sketch_line must have been adopted. No line was
    //    drawn manually, so any sketch_line here came from the Project tool.
    const types = await subCallTypes(page);
    expect(types, "no sketch_line adopted from the projected edge").toContain(
      "sketch_line",
    );

    const adopted = await page.evaluate(() => {
      const calls =
        (
          window as unknown as {
            __fdStore?: {
              state: {
                sketchSession: {
                  subCalls: Array<{
                    type: string;
                    params?: Record<string, unknown>;
                  }>;
                };
              };
            };
          }
        ).__fdStore?.state.sketchSession.subCalls ?? [];
      return calls.some(
        (c) => c.type === "sketch_line" && c.params?.construction === true,
      );
    });
    expect(
      adopted,
      "adopted line is not flagged construction",
    ).toBe(true);

    // 5) Finish and assert a clean page.
    await page.getByTestId("sketch-finish").click();
    await page.waitForTimeout(300);
    expect(pageErrors, `page errors: ${pageErrors.join("; ")}`).toEqual([]);
  });
});
