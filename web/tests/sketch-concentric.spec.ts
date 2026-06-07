import { expect, test } from "@playwright/test";

/**
 * Concentric constraint via the palette. The solver, draft-state rebuild,
 * palette logic, and param mapping are all covered by Vitest / cargo; this
 * E2E proves the interactive path: draw two circles at different centres,
 * drop to Select mode, pick both circles, and confirm the palette surfaces
 * the Concentric button. Clicking it must push a `sketch_constraint_concentric`
 * sub-call into the live session and survive a re-solve. Cold WebGPU
 * first-paint is slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

/**
 * Settle the live ortho camera and return a sketch→screen projector that
 * mirrors `sketchToScreen()` with the overlay's measured viewport. The R3F
 * camera publishes its zoom asynchronously after the WebGPU canvas mounts,
 * so poll until two reads a frame apart agree and the camera has published.
 */
async function makeProjector(page: import("@playwright/test").Page) {
  const overlay = page.getByTestId("sketch-canvas-2d");
  const box = await overlay.boundingBox();
  expect(box).not.toBeNull();
  const readCam = () =>
    page.evaluate(() => {
      const hook = (
        window as unknown as {
          __fdSketchCamera?: () => {
            zoom: number;
            centerU: number;
            centerV: number;
            version: number;
          };
        }
      ).__fdSketchCamera;
      const c = hook?.();
      return c
        ? { zoom: c.zoom, centerU: c.centerU, centerV: c.centerV, version: c.version }
        : null;
    });
  let cam = await readCam();
  for (let i = 0; i < 40; i++) {
    await page.waitForTimeout(100);
    const next = await readCam();
    if (
      next &&
      cam &&
      next.version > 0 &&
      next.zoom === cam.zoom &&
      next.centerU === cam.centerU &&
      next.centerV === cam.centerV
    ) {
      cam = next;
      break;
    }
    cam = next;
  }
  expect(cam).not.toBeNull();
  expect(cam!.version).toBeGreaterThan(0);
  const b = box!;
  const c = cam!;
  return (u: number, v: number): [number, number] => {
    const px = b.x + b.width / 2 + (u - c.centerU) * c.zoom;
    const py = b.y + b.height / 2 - (v - c.centerV) * c.zoom;
    return [px, py];
  };
}

/** Draw a circle by dragging centre→rim, using the projector so the
 *  centre / radius land on known sketch coordinates. */
async function drawCircle(
  page: import("@playwright/test").Page,
  project: (u: number, v: number) => [number, number],
  center: [number, number],
  radiusMm: number,
) {
  await page.getByTestId("sketch-tool-sketch_circle").click();
  const [csx, csy] = project(center[0], center[1]);
  const [rsx, rsy] = project(center[0] + radiusMm, center[1]);
  await page.mouse.move(csx, csy);
  await page.mouse.down();
  await page.mouse.move(csx + 8, csy + 4, { steps: 5 });
  await page.mouse.move(rsx, rsy, { steps: 10 });
  await page.mouse.up();
}

test.describe("Sketch concentric constraint", () => {
  test("pick two circles, palette offers Concentric, applying it persists", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    const project = await makeProjector(page);

    // Two circles at clearly different centres so they're never coincident
    // to begin with — concentric is a real change the solver must apply.
    const c1: [number, number] = [-40, 20];
    const r1 = 30;
    const c2: [number, number] = [50, -30];
    const r2 = 20;
    await drawCircle(page, project, c1, r1);
    await drawCircle(page, project, c2, r2);

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);
    await page.waitForTimeout(400);

    // Drop to select mode and pick both circles on their rims (the circle
    // hit-test matches a click whose distance to the centre ≈ radius).
    await page.getByTestId("sketch-tool-select").click();
    const [p1x, p1y] = project(c1[0] + r1, c1[1]); // rim of circle 1
    const [p2x, p2y] = project(c2[0] + r2, c2[1]); // rim of circle 2
    await page.mouse.click(p1x, p1y);
    await page.waitForTimeout(120); // let the store/overlay settle the pick
    await page.mouse.click(p2x, p2y);
    await page.waitForTimeout(120);

    // The palette must surface the Concentric button for two circles.
    await expect(page.getByTestId("sketch-constraint-concentric")).toBeVisible();

    // Apply it — a third sub-spine row (the constraint) appears.
    await page.getByTestId("sketch-constraint-concentric").click();
    await expect(subSpineRows).toHaveCount(3);
    await page.waitForTimeout(800); // let the solver land

    // The live session now carries a concentric sub-call.
    const hasConcentric = await page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            state: {
              sketchSession: { subCalls: Array<{ type: string }> };
            };
          };
        }
      ).__fdStore;
      const calls = store?.state.sketchSession.subCalls ?? [];
      return calls.some((c) => c.type === "sketch_constraint_concentric");
    });
    expect(hasConcentric).toBe(true);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{ type: string }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });
    expect(committed).not.toBeNull();
    expect(
      committed!.some((c) => c.type === "sketch_constraint_concentric"),
    ).toBe(true);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
