import { expect, test } from "@playwright/test";

/**
 * K6b.2/K6b.3 smoke tests for the sketch editor.
 *
 * Runs headed Chromium with the host's real WebGPU stack (see
 * `playwright.config.ts`). The point is not exhaustive coverage —
 * Vitest already owns the unit-level checks for schema parsing,
 * reducers and the fdApi façade. What Vitest can't catch is
 * integration bugs that only surface when the scene actually
 * renders: material/node-material incompatibility, drawIndexed
 * crashes, geometry shapes that confuse the WebGPU backend.
 *
 * Every test here therefore attaches a `pageerror` + `console` error
 * listener and asserts the list is empty at the end. The
 * LineMaterial / NodeMaterial compatibility regression we hit in
 * K6b.2 would trip the drawIndexed + NodeMaterial filters below
 * immediately.
 */

/** Console error substrings we ignore — they come from third-party
 *  libraries or the WebGPU adapter probing and aren't actionable. */
const IGNORED_CONSOLE_SUBSTRINGS = [
  "WebGPU",
  "webgl",
  "THREE.Clock",
];

function isBlockingConsoleMessage(text: string): boolean {
  const lower = text.toLowerCase();
  return !IGNORED_CONSOLE_SUBSTRINGS.some((s) => lower.includes(s.toLowerCase()));
}

/* ------------------------------------------------------------------ */
/*  Shared sketch-overlay helpers (P0.1 / P0.2)                        */
/* ------------------------------------------------------------------ */

/** Read the live sketch camera and return a projector that maps a
 *  sketch coordinate to absolute client pixels on the overlay. */
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
  // The R3F ortho camera publishes its zoom asynchronously after the
  // WebGPU canvas mounts. Project with a *settled* camera so the click
  // maps to the same sketch coordinate the overlay hit-test will use:
  // poll until two reads a frame apart agree and the camera has
  // actually published (version > 0).
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
  // Mirror sketchToScreen() with the overlay's own measured size as
  // the authoritative viewport (the bridge can publish stale dims).
  return (u: number, v: number): [number, number] => {
    const px = b.x + b.width / 2 + (u - c.centerU) * c.zoom;
    const py = b.y + b.height / 2 - (v - c.centerV) * c.zoom;
    return [px, py];
  };
}

/** Draw one line via the FeatureDialog at precise coordinates. */
async function drawLine(
  page: import("@playwright/test").Page,
  start: [number, number],
  end: [number, number],
) {
  await page.getByTestId("sketch-tool-sketch_line").click();
  await page.getByLabel("Start x").fill(String(start[0]));
  await page.getByLabel("Start y").fill(String(start[1]));
  await page.getByLabel("End x").fill(String(end[0]));
  await page.getByLabel("End y").fill(String(end[1]));
  await page.getByTestId("sketch-tool-apply").click();
}

function readCommittedSubCalls(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const kernel = (
      window as unknown as {
        __fdKernel?: {
          listFeatures: () => Array<{ id: number; kind: string }>;
          getSketchSubCalls: (id: number) => Array<{
            type: string;
            params: Record<string, unknown>;
          }>;
        };
      }
    ).__fdKernel;
    if (!kernel) return null;
    const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
    if (!sketch) return null;
    return kernel.getSketchSubCalls(sketch.id);
  });
}

test.describe("Sketch editor smoke", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
  });

  test("enter sketch mode, draw a line via the dialog, render it", async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: Error[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
    await page.waitForTimeout(600);

    // 1. Enter sketch mode. The 2D sketch overlay canvas must
    //    appear immediately as a sibling of the WebGPU canvas.
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-mode-indicator")).toBeVisible();
    await expect(page.getByTestId("sketch-session-panel")).toBeVisible();
    await expect(page.getByTestId("sketch-session-empty")).toBeVisible();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    // 2. Pick the line tool. The outliner should now show the
    //    generic FeatureDialog for sketch_line with Start / End
    //    vec2 widgets.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expect(page.getByTestId("sketch-session-empty")).toHaveCount(0);

    // 3. Fill the coordinates. Vec2Widget uses aria-labels built
    //    from the param display name: "Start x", "Start y",
    //    "End x", "End y".
    await page.getByLabel("Start x").fill("-50");
    await page.getByLabel("Start y").fill("-50");
    await page.getByLabel("End x").fill("50");
    await page.getByLabel("End y").fill("50");

    // 4. Apply the draft. The sub-spine list must grow by one.
    await page.getByTestId("sketch-tool-apply").click();
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    // 5. Give the renderer a few frames to draw the new line and
    //    surface any WebGPU draw-time crashes.
    await page.waitForTimeout(500);

    // 6. Finish the sketch — K6b.3a commits the session as a
    //    real `sketch` FeatureCall on the kernel, so the
    //    outliner grows a new row with `data-feature-op="union"`
    //    and the feature-count badge goes from 0 to 1. The
    //    overlay canvas unmounts, the regular toolbar comes
    //    back.
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("sketch-session-panel")).toHaveCount(0);
    await expect(page.getByTestId("sketch-canvas-2d")).toHaveCount(0);
    await expect(page.getByTestId("primitive-split-button")).toBeVisible();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    // The new row's kind glyph is the sketch pencil. (Datum rows like
    // Origin precede feature rows now, so target the sketch row by kind.)
    const sketchRow = page.locator('.fd-outliner .row[data-feature-kind="sketch"]');
    await expect(sketchRow).toContainText("Sketch");

    // 7. No blocking console errors, no uncaught page errors.
    //    This is the guard that would have caught the
    //    LineMaterial / drawIndexed regression.
    const blocking = consoleErrors.filter(isBlockingConsoleMessage);
    expect(
      blocking,
      `unexpected console errors:\n${blocking.join("\n")}`,
    ).toEqual([]);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("line drag snaps its start to the sketch origin", async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: Error[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    // Enter sketch mode. The origin lives at (0, 0) in sketch
    // space and is drawn right in the centre of the overlay at
    // the default ortho zoom. We start the drag within the
    // 10 mm snap radius of that point so the K5b inference
    // engine should pull the line's start exactly onto (0, 0)
    // regardless of cursor jitter.
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    // Canvas centre maps to sketch (0, 0) under the K6b.2 ortho
    // camera setup (position [0, 500, 0], centre-lock on
    // origin). A few px offset exercises the snap; at zoom 1.2
    // one screen pixel equals ~0.83 mm, so 6 px ≈ 5 mm, well
    // inside the 10 mm snap radius.
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    const endX = cx + 120;
    const endY = cy - 60;

    await page.mouse.move(cx + 4, cy + 4);
    await page.mouse.down();
    await page.mouse.move(cx + 4, cy + 4, { steps: 2 });
    await page.mouse.move(endX, endY, { steps: 10 });
    await page.mouse.up();

    // K6b.3d: a start-snap to origin now commits both the
    // `sketch_line` call and a `sketch_constraint_coincident`
    // referencing the origin entity.
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);

    // Let the commit-time solver land before reading back.
    await page.waitForTimeout(700);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const features = kernel.listFeatures();
      const sketch = features.find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!.length).toBe(2);
    const lineCall = committed![0] as {
      type: string;
      params: { start: [number, number]; end: [number, number] };
    };
    expect(lineCall.type).toBe("sketch_line");
    const [sx, sy] = lineCall.params.start;
    expect(sx).toBeCloseTo(0, 5);
    expect(sy).toBeCloseTo(0, 5);
    // Second entry must be the coincident-to-origin constraint.
    const cstrCall = committed![1] as { type: string };
    expect(cstrCall.type).toBe("sketch_constraint_coincident");

    const blocking = consoleErrors.filter(isBlockingConsoleMessage);
    expect(
      blocking,
      `unexpected console errors:\n${blocking.join("\n")}`,
    ).toEqual([]);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("double-click a sketch row to re-open it and append another line", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    // Create the first sketch with one line.
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.getByLabel("Start x").fill("10");
    await page.getByLabel("Start y").fill("10");
    await page.getByLabel("End x").fill("40");
    await page.getByLabel("End y").fill("10");
    await page.getByTestId("sketch-tool-apply").click();
    await page.getByTestId("sketch-finish").click();

    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    const row = page.locator('.fd-outliner .row[data-feature-kind="sketch"]');
    await expect(row).toHaveCount(1);

    // K6b.3b: double-click reopens the committed sketch. The
    // sub-spine panel must already show the previously applied
    // line and the overlay canvas comes back up.
    await row.dblclick();
    await expect(page.getByTestId("sketch-session-panel")).toBeVisible();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    const initialRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(initialRows).toHaveCount(1);

    // Append a second line.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await page.getByLabel("Start x").fill("10");
    await page.getByLabel("Start y").fill("20");
    await page.getByLabel("End x").fill("40");
    await page.getByLabel("End y").fill("20");
    await page.getByTestId("sketch-tool-apply").click();
    await expect(initialRows).toHaveCount(2);

    await page.getByTestId("sketch-finish").click();

    // Feature count unchanged — replaceSketch kept the id stable.
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const features = kernel.listFeatures();
      const sketch = features.find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!).toHaveLength(2);
    expect(committed![0].type).toBe("sketch_line");
    expect(committed![1].type).toBe("sketch_line");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("near-horizontal drag persists a horizontal constraint and solves the line flat", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();

    // Drag a near-horizontal line (small vertical wobble) from
    // off-centre to off-centre so neither endpoint lands inside
    // the origin snap radius. The horizontal inference should
    // still fire because the vertical deviation is below the
    // 10 mm snap threshold, and the commit-time solver should
    // then flatten the line to end.y == start.y.
    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    // Zero-wobble horizontal drag — the mouse lands on pixel
    // boundaries so the inferred vertical delta is reliably
    // below the 10 mm snap radius no matter what device
    // pixel ratio the test host uses.
    const startX = cx - 120;
    const endX = cx + 120;
    const y = cy + 120;

    await page.mouse.move(startX, y);
    await page.mouse.down();
    await page.mouse.move((startX + endX) / 2, y, { steps: 5 });
    await page.mouse.move(endX, y, { steps: 10 });
    await page.mouse.up();

    // The sub-spine grows by two entries: the line and the
    // horizontal constraint.
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);

    // Give the async solver time to land its result.
    await page.waitForTimeout(800);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const features = kernel.listFeatures();
      const sketch = features.find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!.length).toBe(2);
    const line = committed![0] as {
      type: string;
      params: { start: [number, number]; end: [number, number] };
    };
    const constraint = committed![1] as {
      type: string;
      params: { line: number };
    };
    expect(line.type).toBe("sketch_line");
    expect(constraint.type).toBe("sketch_constraint_horizontal");
    // The solver must have flattened the line.
    expect(line.params.end[1]).toBeCloseTo(line.params.start[1], 5);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("rectangle tool drags a rectangle and solves every side axis-aligned", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_rectangle").click();

    // Drag a rectangle across the canvas. The rectangle tool's
    // internal H/V constraints are independent of the cursor
    // direction, so even a diagonal drag should land as an
    // axis-aligned rectangle after the commit-time solver runs.
    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    const startX = cx - 120;
    const startY = cy - 60;
    const endX = cx + 100;
    const endY = cy + 80;

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 5 });
    await page.mouse.move(endX, endY, { steps: 10 });
    await page.mouse.up();

    // One sub-spine entry: the rectangle call. All four
    // constraints are internal to the tool, not separate rows.
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    await page.waitForTimeout(800);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const features = kernel.listFeatures();
      const sketch = features.find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!.length).toBe(1);
    const rect = committed![0] as {
      type: string;
      params: { corner_a: [number, number]; corner_b: [number, number] };
    };
    expect(rect.type).toBe("sketch_rectangle");
    // After solving, bl = (min_x, min_y) and tr = (max_x, max_y)
    // — the rectangle's extents are non-degenerate and the
    // corners are ordered so the width and height are positive.
    const [ax, ay] = rect.params.corner_a;
    const [bx, by] = rect.params.corner_b;
    expect(bx - ax).toBeGreaterThan(0);
    expect(by - ay).toBeGreaterThan(0);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("circle tool drags a circle and persists its centre + radius", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_circle").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    // Centre the drag at ~(80, 80) sketch mm and pull a ~50 mm
    // radius. At zoom 1.2 that's 60 screen pixels.
    const centerSx = cx + 96;
    const centerSy = cy + 96;
    const rimSx = centerSx + 60;
    const rimSy = centerSy;

    await page.mouse.move(centerSx, centerSy);
    await page.mouse.down();
    await page.mouse.move(centerSx + 20, centerSy + 10, { steps: 5 });
    await page.mouse.move(rimSx, rimSy, { steps: 10 });
    await page.mouse.up();

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    await page.waitForTimeout(700);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const features = kernel.listFeatures();
      const sketch = features.find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!.length).toBe(1);
    const circle = committed![0] as {
      type: string;
      params: { center: [number, number]; radius: number };
    };
    expect(circle.type).toBe("sketch_circle");
    expect(circle.params.radius).toBeGreaterThan(10);
    // Centre's x should be to the right of the canvas origin
    // (positive in sketch coords).
    expect(circle.params.center[0]).toBeGreaterThan(0);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("per-sub-call delete removes one entry and leaves the rest intact", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    // Draw a rectangle and a circle in the same session. The
    // sub-spine panel ends up with two rows.
    await page.getByTestId("enter-sketch").click();
    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    await page.getByTestId("sketch-tool-sketch_rectangle").click();
    await page.mouse.move(cx - 150, cy - 60);
    await page.mouse.down();
    await page.mouse.move(cx - 30, cy + 60, { steps: 10 });
    await page.mouse.up();

    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.mouse.move(cx + 80, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 140, cy, { steps: 10 });
    await page.mouse.up();

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);
    await page.waitForTimeout(700);

    // K6b.3g: delete the circle (second row). The per-row delete
    // button is `pointer-events:none` until its row is hovered (the
    // "quiet row" UX), so mirror what a user does — hover the row to
    // reveal + enable the button, then click it.
    await subSpineRows.nth(1).hover();
    await page.getByTestId("sketch-sub-row-delete-1").click();
    await expect(subSpineRows).toHaveCount(1);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const features = kernel.listFeatures();
      const sketch = features.find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    expect(committed!.length).toBe(1);
    expect(committed![0].type).toBe("sketch_rectangle");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K7.4: rectangle → finish → extrude → STL download end-to-end", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    // Extrude button starts disabled — no sketch to consume.
    const extrudeBtn = page.getByTestId("extrude-latest-sketch");
    await expect(extrudeBtn).toBeDisabled();

    // Draw a rectangle via the dialog path (precise coords).
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_rectangle").click();
    await page.getByLabel("Corner a x").fill("-20");
    await page.getByLabel("Corner a y").fill("-10");
    await page.getByLabel("Corner b x").fill("20");
    await page.getByLabel("Corner b y").fill("10");
    await page.getByTestId("sketch-tool-apply").click();
    await page.waitForTimeout(400);
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    // Extrude button enabled after the sketch commits.
    await expect(extrudeBtn).toBeEnabled();
    await extrudeBtn.click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    // Verify the new feature is an extrude via the dev kernel hook.
    const shape = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string; name: string }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      return kernel.listFeatures();
    });
    expect(shape).not.toBeNull();
    expect(shape!.some((f) => f.kind === "sketch")).toBe(true);
    expect(shape!.some((f) => f.kind === "extrude")).toBe(true);

    // Download the STL and check it contains real triangles. Export
    // now lives in the ☰ settings menu under an Export submenu:
    // open the menu, hover Export to reveal the submenu, click STL.
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      (async () => {
        await page.getByTestId("settings-menu-trigger").click();
        await page.getByTestId("menu-export").hover();
        await page.getByTestId("menu-export-stl").click();
      })(),
    ]);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    // Header (80) + count (4) + several hundred triangles at 50
    // bytes each — much larger than the 84-byte empty stub.
    expect(stat.size).toBeGreaterThan(1000);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("enter sketch mode, drag a line on the 2D overlay", async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: Error[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();

    // Drag on the 2D overlay canvas. pointerdown → move → up
    // drives the new drag-to-draw path in SketchCanvas2d. We
    // don't assert resolved sketch-space coordinates because
    // those depend on the ortho camera zoom — we only care
    // that (a) one sub-spine entry is produced, (b) nothing
    // crashes.
    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const startX = box.x + box.width / 2 - 80;
    const startY = box.y + box.height / 2 - 40;
    const endX = box.x + box.width / 2 + 80;
    const endY = box.y + box.height / 2 + 40;

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    // A couple of intermediate moves exercise the live-preview
    // path in SketchCanvas2d's RAF loop.
    await page.mouse.move(startX + 20, startY + 10, { steps: 5 });
    await page.mouse.move(endX, endY, { steps: 10 });
    await page.mouse.up();

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    await page.waitForTimeout(400);

    const blocking = consoleErrors.filter(isBlockingConsoleMessage);
    expect(
      blocking,
      `unexpected console errors:\n${blocking.join("\n")}`,
    ).toEqual([]);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});

/**
 * P0.1 — explicit constraint palette.
 *
 * Vitest already covers the pure pieces (hit-test, applicability,
 * param mapping, reducers, solve-replay). What only surfaces in a
 * real browser is the integration: the Select tool actually drops
 * back to select mode, a left-click on the overlay hit-tests the
 * committed geometry and drives the store selection, the palette
 * renders the right buttons for that pick, and clicking one applies
 * the constraint, persists it on the kernel, and the live solver
 * actually moves the geometry to satisfy it.
 *
 * Entities are placed at known sketch coordinates via the dialog,
 * then targeted by projecting those coordinates through the live
 * ortho camera (`__fdSketchCamera`) — robust against whatever
 * zoom/DPR the headed host happens to use.
 */
test.describe("Sketch constraint palette (P0.1)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
  });

  test("Select tool returns to select mode and shows the constraint hint", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // Picking a drawing tool leaves select mode — the palette hides.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expect(page.getByTestId("sketch-constraint-row")).toHaveCount(0);

    // The Select tool drops back to select mode. The palette returns
    // with its empty-selection hint, and the Select button reads active.
    await page.getByTestId("sketch-tool-select").click();
    const row = page.getByTestId("sketch-constraint-row");
    await expect(row).toBeVisible();
    await expect(row).toContainText("Select geometry");
    await expect(page.getByTestId("sketch-tool-select")).toHaveClass(/is-active/);

    // Escape from a drawing tool also returns to select mode.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expect(page.getByTestId("sketch-constraint-row")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("sketch-constraint-row")).toBeVisible();

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("pick two lines, apply Parallel, solver makes them parallel", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();

    // Two clearly non-parallel lines, both off the origin so neither
    // endpoint snaps and the midpoints are unambiguous pick targets.
    await drawLine(page, [20, 30], [80, 40]); // slope +1/6
    await drawLine(page, [20, -40], [80, 0]); // slope +2/3
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);

    // Drop to select mode and pick both lines on their midpoints.
    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [m1x, m1y] = project(50, 35); // midpoint of line 1
    const [m2x, m2y] = project(50, -20); // midpoint of line 2
    await page.mouse.click(m1x, m1y);
    await page.waitForTimeout(120); // let the store/overlay settle the pick
    await page.mouse.click(m2x, m2y);

    // The palette now offers Parallel + Perpendicular for two lines.
    await expect(page.getByTestId("sketch-constraint-parallel")).toBeVisible();
    await expect(page.getByTestId("sketch-constraint-perpendicular")).toBeVisible();

    // Apply Parallel — a third sub-spine row (the constraint) appears.
    await page.getByTestId("sketch-constraint-parallel").click();
    await expect(subSpineRows).toHaveCount(3);
    await page.waitForTimeout(800); // let the solver land

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const lines = committed!.filter(
      (c) => c.type === "sketch_line",
    ) as unknown as Array<{
      params: { start: [number, number]; end: [number, number] };
    }>;
    expect(lines.length).toBe(2);
    expect(
      committed!.some((c) => c.type === "sketch_constraint_parallel"),
    ).toBe(true);

    // The two line directions must now be parallel: their 2D cross
    // product is ~0.
    const dir = (l: { params: { start: [number, number]; end: [number, number] } }) => [
      l.params.end[0] - l.params.start[0],
      l.params.end[1] - l.params.start[1],
    ];
    const [ax, ay] = dir(lines[0]);
    const [bx, by] = dir(lines[1]);
    const cross = ax * by - ay * bx;
    const norm = Math.hypot(ax, ay) * Math.hypot(bx, by);
    expect(Math.abs(cross) / norm).toBeLessThan(1e-3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("pick two points, apply Coincident, solver merges them", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();

    // Two free points a short distance apart, both off the origin.
    await page.getByTestId("sketch-tool-sketch_point").click();
    await page.getByLabel("At x").fill("40");
    await page.getByLabel("At y").fill("40");
    await page.getByTestId("sketch-tool-apply").click();
    await page.getByTestId("sketch-tool-sketch_point").click();
    await page.getByLabel("At x").fill("70");
    await page.getByLabel("At y").fill("55");
    await page.getByTestId("sketch-tool-apply").click();

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [p1x, p1y] = project(40, 40);
    const [p2x, p2y] = project(70, 55);
    await page.mouse.click(p1x, p1y);
    await page.waitForTimeout(120); // let the store/overlay settle the pick
    await page.mouse.click(p2x, p2y);

    await expect(page.getByTestId("sketch-constraint-coincident")).toBeVisible();
    await page.getByTestId("sketch-constraint-coincident").click();
    await expect(subSpineRows).toHaveCount(3);
    await page.waitForTimeout(800);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    expect(
      committed!.some((c) => c.type === "sketch_constraint_coincident"),
    ).toBe(true);
    const points = committed!.filter(
      (c) => c.type === "sketch_point",
    ) as unknown as Array<{
      params: { at: [number, number] };
    }>;
    expect(points.length).toBe(2);
    // Solved coincident: the two points sit on top of each other.
    expect(points[0].params.at[0]).toBeCloseTo(points[1].params.at[0], 3);
    expect(points[0].params.at[1]).toBeCloseTo(points[1].params.at[1], 3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("clicking empty space clears the selection", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await drawLine(page, [20, 30], [80, 40]);

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [mx, my] = project(50, 35);
    await page.mouse.click(mx, my);
    // One line selected → it offers Horizontal/Vertical.
    await expect(page.getByTestId("sketch-constraint-horizontal")).toBeVisible();

    // Click far-off empty space → selection clears, palette shows the
    // empty hint again.
    const [ex, ey] = project(-90, -90);
    await page.waitForTimeout(120);
    await page.mouse.click(ex, ey);
    await expect(page.getByTestId("sketch-constraint-horizontal")).toHaveCount(0);
    await expect(page.getByTestId("sketch-constraint-row")).toContainText(
      "Select geometry",
    );

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});

/**
 * P0.2 — editable dimensions.
 *
 * The palette seeds Length/Distance and Angle dimensions from the
 * current measurement (so applying never makes the sketch jump), and
 * the constraint manager exposes the value as an editable field that
 * re-drives the geometry through the live solver. These checks need a
 * real browser: the value edit only takes effect once the async
 * planegcs solve lands and rewrites the persisted sub-calls.
 */
test.describe("Sketch dimensions (P0.2)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
  });

  test("length dimension on a line drives its length when edited", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // A 40 mm horizontal line, off the origin.
    await drawLine(page, [10, 0], [50, 0]);

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [mx, my] = project(30, 0); // line midpoint
    await page.mouse.click(mx, my);

    // The palette offers a Length dimension for a single line.
    const lengthBtn = page.getByTestId("sketch-constraint-distance");
    await expect(lengthBtn).toBeVisible();
    await lengthBtn.click();

    // The constraint manager now shows the measured value (≈40) in an
    // editable field. The line is sub-call 0, the distance is 1.
    const valueInput = page.getByTestId("constraint-value-1");
    await expect(valueInput).toBeVisible();
    await expect(valueInput).toHaveValue("40");

    // Drive it to 80 mm and let the solver land.
    await valueInput.fill("80");
    await valueInput.press("Enter");
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const dist = committed!.find(
      (c) => c.type === "sketch_constraint_distance",
    ) as { params: { value_mm: number } } | undefined;
    expect(dist).toBeDefined();
    expect(dist!.params.value_mm).toBeCloseTo(80, 3);

    // The solved line is actually 80 mm long.
    const line = committed!.find((c) => c.type === "sketch_line") as unknown as {
      params: { start: [number, number]; end: [number, number] };
    };
    const len = Math.hypot(
      line.params.end[0] - line.params.start[0],
      line.params.end[1] - line.params.start[1],
    );
    expect(len).toBeCloseTo(80, 2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("angle dimension drives two lines to the edited angle", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await drawLine(page, [10, 0], [50, 0]); // line A, horizontal
    await drawLine(page, [10, 20], [50, 40]); // line B, ~26.6°

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [ax, ay] = project(30, 0); // midpoint of line A
    const [bx, by] = project(30, 30); // midpoint of line B
    await page.mouse.click(ax, ay);
    await page.waitForTimeout(120);
    await page.mouse.click(bx, by);

    const angleBtn = page.getByTestId("sketch-constraint-angle");
    await expect(angleBtn).toBeVisible();
    await angleBtn.click();

    // Lines are sub-calls 0 and 1; the angle constraint is 2.
    const valueInput = page.getByTestId("constraint-value-2");
    await expect(valueInput).toBeVisible();
    await valueInput.fill("90");
    await valueInput.press("Enter");
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const ang = committed!.find(
      (c) => c.type === "sketch_constraint_angle",
    ) as { params: { value_deg: number } } | undefined;
    expect(ang).toBeDefined();
    expect(ang!.params.value_deg).toBeCloseTo(90, 3);

    // The two solved lines are perpendicular: direction dot product ~0.
    const lines = committed!.filter(
      (c) => c.type === "sketch_line",
    ) as unknown as Array<{
      params: { start: [number, number]; end: [number, number] };
    }>;
    expect(lines.length).toBe(2);
    const dir = (l: { params: { start: [number, number]; end: [number, number] } }) => [
      l.params.end[0] - l.params.start[0],
      l.params.end[1] - l.params.start[1],
    ];
    const [dax, day] = dir(lines[0]);
    const [dbx, dby] = dir(lines[1]);
    const dot = dax * dbx + day * dby;
    const norm = Math.hypot(dax, day) * Math.hypot(dbx, dby);
    expect(Math.abs(dot) / norm).toBeLessThan(1e-3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});

/**
 * P0.3 — drag-edit committed geometry via grip handles.
 *
 * Selecting an entity exposes grip handles at its key points; dragging
 * a grip rewrites the underlying coordinate live and the solver
 * re-runs on drop. Only a real browser exercises the full chain:
 * select-mode hit-test → grip pick → pointer-capture drag →
 * replaceSketchSubCalls per move → solveAndReplace on release.
 */
test.describe("Sketch grip-drag (P0.3)", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
  });

  function readSketchSession(page: import("@playwright/test").Page) {
    return page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            getState: () => {
              sketchSession: {
                active: boolean;
                selection: ReadonlyArray<number>;
                subCalls: Array<{ type: string; params: Record<string, unknown> }>;
              };
            };
          };
        }
      ).__fdStore;
      if (!store) return null;
      const s = store.getState().sketchSession;
      return { active: s.active, selection: [...s.selection], subCalls: s.subCalls };
    });
  }

  test("drag a line endpoint to relocate it", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // A 40 mm horizontal line, off the origin so its endpoints don't
    // collide with the origin grip.
    await drawLine(page, [10, 0], [50, 0]);

    // Select the line by clicking its midpoint. The palette's
    // single-line options confirm the line is selected (and so its
    // endpoint grips are now live).
    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [mx, my] = project(30, 0);
    await page.mouse.click(mx, my);
    await expect(page.getByTestId("sketch-constraint-horizontal")).toBeVisible();

    // Grab the end-point grip at (50, 0) and drag it to (50, 40). The
    // line is unconstrained, so the solver leaves the dragged point put.
    const [ex, ey] = project(50, 0);
    const [tx, ty] = project(50, 40);
    await page.mouse.move(ex, ey);
    await page.mouse.down();
    await page.mouse.move((ex + tx) / 2, (ey + ty) / 2, { steps: 5 });
    await page.mouse.move(tx, ty, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const line = committed!.find((c) => c.type === "sketch_line") as unknown as {
      params: { start: [number, number]; end: [number, number] };
    };
    expect(line).toBeDefined();
    // Start unchanged, end relocated to ~(50, 40) within pixel-rounding.
    expect(Math.abs(line.params.start[0] - 10)).toBeLessThan(2);
    expect(Math.abs(line.params.start[1] - 0)).toBeLessThan(2);
    expect(Math.abs(line.params.end[0] - 50)).toBeLessThan(2);
    expect(Math.abs(line.params.end[1] - 40)).toBeLessThan(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("drag a circle rim grip to change its radius", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // A circle at (40, 0) with radius 20, placed precisely via the dialog.
    await page.getByTestId("sketch-tool-sketch_circle").click();
    await page.getByLabel("Center x").fill("40");
    await page.getByLabel("Center y").fill("0");
    await page.getByLabel("Radius").fill("20");
    await page.getByTestId("sketch-tool-apply").click();

    // Select the circle by clicking its rightmost rim point (60, 0).
    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [rimx, rimy] = project(60, 0);
    await page.mouse.click(rimx, rimy);

    // Confirm the circle is selected so the rim grip is live.
    const sel = await readSketchSession(page);
    expect(sel).not.toBeNull();
    expect(sel!.selection.length).toBeGreaterThan(0);

    // Drag the rim grip from (60, 0) out to (80, 0): radius 20 → 40.
    const [tx, ty] = project(80, 0);
    await page.mouse.move(rimx, rimy);
    await page.mouse.down();
    await page.mouse.move((rimx + tx) / 2, rimy, { steps: 5 });
    await page.mouse.move(tx, ty, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const circle = committed!.find((c) => c.type === "sketch_circle") as unknown as {
      params: { center: [number, number]; radius: number };
    };
    expect(circle).toBeDefined();
    // Centre stays put; radius grew to ~40.
    expect(Math.abs(circle.params.center[0] - 40)).toBeLessThan(2);
    expect(Math.abs(circle.params.center[1] - 0)).toBeLessThan(2);
    expect(Math.abs(circle.params.radius - 40)).toBeLessThan(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
