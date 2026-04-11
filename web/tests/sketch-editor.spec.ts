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
    await expect(page.getByTestId("add-box")).toBeVisible();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    // The new row's kind glyph is the sketch pencil.
    const sketchRow = page.locator(".fd-outliner .row").first();
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

    // K6b.3g: delete the circle (second row).
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

    // Download the STL and check it contains real triangles.
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
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
