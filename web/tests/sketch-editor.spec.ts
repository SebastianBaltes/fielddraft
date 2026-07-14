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
 *  libraries or the WebGPU adapter probing and aren't actionable.
 *
 *  The 401 belongs to the multi-user backend: the app probes `/api/me` on
 *  boot, and an e2e run carries no session cookie, so 401 IS the correct
 *  answer — the browser just logs every failed fetch as a console error.
 *  Only 401 is waved through; a 5xx still fails the test, because that means
 *  the server is broken or not running rather than merely unauthenticated. */
const IGNORED_CONSOLE_SUBSTRINGS = [
  "WebGPU",
  "webgl",
  "THREE.Clock",
  "the server responded with a status of 401",
];

function isBlockingConsoleMessage(text: string): boolean {
  const lower = text.toLowerCase();
  return !IGNORED_CONSOLE_SUBSTRINGS.some((s) =>
    lower.includes(s.toLowerCase()),
  );
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
  const b = box!;
  const readCam = () =>
    page.evaluate(() => {
      const hook = (
        window as unknown as {
          __fdSketchCamera?: () => { zoom: number; version: number };
        }
      ).__fdSketchCamera;
      const c = hook?.();
      return c ? { zoom: c.zoom, version: c.version } : null;
    });
  // The real viewport camera publishes its plane-frame zoom asynchronously
  // after the WebGPU canvas mounts. Wait for a *settled, finite* zoom so the
  // click maps to the same sketch coordinate the overlay hit-test will use:
  // poll until two reads a frame apart agree, the camera has published
  // (version > 0) and the zoom is finite & positive.
  let cam = await readCam();
  for (let i = 0; i < 40; i++) {
    await page.waitForTimeout(100);
    const next = await readCam();
    if (
      next &&
      cam &&
      next.version > 0 &&
      Number.isFinite(next.zoom) &&
      next.zoom > 0 &&
      next.zoom === cam.zoom
    ) {
      cam = next;
      break;
    }
    cam = next;
  }
  expect(cam).not.toBeNull();
  expect(cam!.version).toBeGreaterThan(0);
  expect(Number.isFinite(cam!.zoom) && cam!.zoom > 0).toBe(true);
  // Project through the REAL camera + plane frame (the exact inverse of the
  // overlay's screen→sketch hit-test) via the bridge's dev hook. Use the
  // BRIDGE's own dims (no override): the app's grip/entity hit-test projects
  // with `ev.cam` = the bridge dims, so the test must too or the projected
  // click lands a hair off each grip — inside the loose SELECT tolerance but
  // OUTSIDE the tight GRIP_PICK_PX, which silently turns an endpoint reshape
  // into a whole-line translate. `__fdSketchToScreen` returns canvas-local CSS
  // px; add the box origin for absolute client px.
  return async (u: number, v: number): Promise<[number, number]> => {
    const local = await page.evaluate(
      ([uu, vv]) => {
        const hook = (
          window as unknown as {
            __fdSketchToScreen?: (u: number, v: number) => [number, number];
          }
        ).__fdSketchToScreen;
        return hook ? hook(uu, vv) : null;
      },
      [u, v] as const,
    );
    expect(local, "__fdSketchToScreen hook missing").not.toBeNull();
    const [lx, ly] = local!;
    expect(
      Number.isFinite(lx) && Number.isFinite(ly),
      `projector produced non-finite px for (${u}, ${v})`,
    ).toBe(true);
    return [b.x + lx, b.y + ly];
  };
}

/** Draw one line via the FeatureDialog at precise coordinates. */
async function drawLine(
  page: import("@playwright/test").Page,
  start: [number, number],
  end: [number, number],
) {
  await pickSketchTool(page, "sketch_line");
  // The per-tool edit block + Apply button live inside the collapsed-by-default
  // "Geometry & Constraints" sub-accordion (ticket 87a1).
  await expandGeometrySection(page);
  await page.getByLabel("Start x").fill(String(start[0]));
  await page.getByLabel("Start y").fill(String(start[1]));
  await page.getByLabel("End x").fill(String(end[0]));
  await page.getByLabel("End y").fill(String(end[1]));
  await page.getByTestId("sketch-tool-apply").click();
}

/**
 * Select a sketch draw tool by its key.
 *
 * Flat tools (line, spline, …) carry the id `sketch-tool-<key>` directly.
 * Rectangle/Circle variants (ticket pqoe) live inside a split-button flyout:
 * their variant id is kept mounted but parked off-screen while the flyout is
 * collapsed, so a direct `.click()` times out as "element is outside of the
 * viewport". The group's primary button invokes the default variant — exactly
 * what a user clicks to draw a default rectangle/circle — so route grouped
 * defaults through it.
 *
 * Idempotent: re-clicking an ALREADY-active tool detaches it back to select
 * mode (ticket daa8 toggle), so a second draw with the same tool would toggle
 * it off and the edit fields would vanish. We therefore click only when the
 * tool (or, for a group, its primary button) isn't already active.
 */
async function pickSketchTool(
  page: import("@playwright/test").Page,
  tool: string,
) {
  const group: Record<string, string> = {
    sketch_rectangle: "sketch-tool-rectangle-group",
    sketch_circle: "sketch-tool-circle-group",
  };
  const groupId = group[tool];
  const button = groupId
    ? page.getByTestId(groupId).locator("button.tool.primary")
    : page.getByTestId(`sketch-tool-${tool}`);
  const cls = (await button.getAttribute("class")) ?? "";
  if (!cls.split(/\s+/).includes("is-active")) await button.click();
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

/**
 * Ticket 87a1: the geometry list, per-tool edit block (Apply/Clear), the
 * empty-state and the constraint manager now live inside the "Geometry &
 * Constraints" sub-accordion (header testid `geometry-header`), COLLAPSED BY
 * DEFAULT inside the generic Detail dialog. Call once after entering the
 * sketch before touching any of those inner elements. Idempotent: only
 * expands when collapsed (chevron ▸ = collapsed, ▾ = expanded).
 */
async function expandGeometrySection(page: import("@playwright/test").Page) {
  const header = page.getByTestId("geometry-header");
  await expect(header).toBeVisible();
  const chevron = header.locator(".accordion-chevron");
  const glyph = (await chevron.textContent())?.trim();
  if (glyph === "▸") {
    await header.click();
    await expect(chevron).toHaveText("▾");
  }
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

  test("enter sketch mode, draw a line via the dialog, render it", async ({
    page,
  }) => {
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
    // Ticket 87a1: the sketch detail now renders inside the generic Detail
    // dialog; its geometry/empty-state/constraints live in the collapsed-by-
    // default "Geometry & Constraints" sub-accordion.
    await expect(page.getByTestId("detail-panel")).toBeVisible();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    await expandGeometrySection(page);
    await expect(page.getByTestId("sketch-session-empty")).toBeVisible();

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
    //    real `sketch` FeatureCall on the kernel, so the outliner
    //    grows a new row und der Feature-Zähler (data-feature-count)
    //    geht von 0 auf 1. The overlay canvas unmounts and the regular toolbar
    //    comes back. The Detail panel itself is a persistent
    //    Outliner section (ticket pqoe) — it does NOT close; the
    //    signal that the session ended is that its in-session ✓
    //    commit button and the 2D overlay are gone.
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.getByTestId("sketch-detail-accept")).toHaveCount(0);
    await expect(page.getByTestId("sketch-canvas-2d")).toHaveCount(0);
    await expect(page.getByTestId("primitive-split-button")).toBeVisible();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    // The new row's kind glyph is the sketch pencil. (Datum rows like
    // Origin precede feature rows now, so target the sketch row by kind.)
    const sketchRow = page.locator(
      '.fd-outliner .row[data-feature-kind="sketch"]',
    );
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
    await expandGeometrySection(page); // ticket 87a1: gated geometry list

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

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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

  test("double-click a sketch row to re-open it and append another line", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    // Create the first sketch with one line.
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expandGeometrySection(page); // ticket 87a1: gated tool dialog + list
    await page.getByLabel("Start x").fill("10");
    await page.getByLabel("Start y").fill("10");
    await page.getByLabel("End x").fill("40");
    await page.getByLabel("End y").fill("10");
    await page.getByTestId("sketch-tool-apply").click();
    await page.getByTestId("sketch-detail-accept").click();

    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    const row = page.locator('.fd-outliner .row[data-feature-kind="sketch"]');
    await expect(row).toHaveCount(1);

    // K6b.3b: double-click reopens the committed sketch. The
    // sub-spine panel must already show the previously applied
    // line and the overlay canvas comes back up.
    await row.dblclick();
    await expect(page.getByTestId("detail-panel")).toBeVisible();
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();
    // Reopening renders a fresh (collapsed) detail panel — expand again.
    await expandGeometrySection(page);
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

    await page.getByTestId("sketch-detail-accept").click();

    // Feature count unchanged — replaceSketch kept the id stable.
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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

  test("near-horizontal drag persists a horizontal constraint and solves the line flat", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expandGeometrySection(page); // ticket 87a1: gated geometry list

    // Drag a near-horizontal line (small vertical wobble) from
    // off-centre to off-centre so neither endpoint lands inside
    // the origin snap radius. The horizontal inference should
    // still fire because the vertical deviation is below the
    // 10 mm snap threshold, and the commit-time solver should
    // then flatten the line to end.y == start.y.
    // Drive the drag through SKETCH coordinates, not raw screen pixels: since
    // the sketch-in-3D rebuild the overlay projects through the real camera, so
    // a screen-horizontal drag is NOT sketch-horizontal. A line with constant
    // v (Δv == 0) is exactly horizontal in sketch space → the commit-time
    // horizontal inference fires. Both endpoints sit 50 mm out on v = 30, well
    // clear of the origin snap radius, and the line never crosses the origin.
    const project = await makeProjector(page);
    const [startX, startY] = await project(-50, 30);
    const [midX, midY] = await project(0, 30);
    const [endX, endY] = await project(50, 30);

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(midX, midY, { steps: 5 });
    await page.mouse.move(endX, endY, { steps: 10 });
    await page.mouse.up();

    // The sub-spine grows by two entries: the line and the
    // horizontal constraint.
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);

    // Give the async solver time to land its result.
    await page.waitForTimeout(800);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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

  test("rectangle tool drags a rectangle and solves every side axis-aligned", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await pickSketchTool(page, "sketch_rectangle");
    await expandGeometrySection(page); // ticket 87a1: gated geometry list

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

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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

  test("circle tool drags a circle and persists its centre + radius", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await pickSketchTool(page, "sketch_circle");
    await expandGeometrySection(page); // ticket 87a1: gated geometry list

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

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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

  test("per-sub-call delete removes one entry and leaves the rest intact", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    // Draw a rectangle and a circle in the same session. The
    // sub-spine panel ends up with two rows.
    await page.getByTestId("enter-sketch").click();
    await expandGeometrySection(page); // ticket 87a1: gated geometry list + delete
    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    await pickSketchTool(page, "sketch_rectangle");
    await page.mouse.move(cx - 150, cy - 60);
    await page.mouse.down();
    await page.mouse.move(cx - 30, cy + 60, { steps: 10 });
    await page.mouse.up();

    await pickSketchTool(page, "sketch_circle");
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

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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

  test("K7.4: rectangle → finish → extrude → STL download end-to-end", async ({
    page,
  }) => {
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
    await pickSketchTool(page, "sketch_rectangle");
    await expandGeometrySection(page); // ticket 87a1: gated tool dialog + apply
    await page.getByLabel("Corner a x").fill("-20");
    await page.getByLabel("Corner a y").fill("-10");
    await page.getByLabel("Corner b x").fill("20");
    await page.getByLabel("Corner b y").fill("10");
    await page.getByTestId("sketch-tool-apply").click();
    await page.waitForTimeout(400);
    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    // Extrude button enabled after the sketch commits.
    await expect(extrudeBtn).toBeEnabled();
    await extrudeBtn.click();
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    // Verify the new feature is an extrude via the dev kernel hook.
    const shape = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{
              id: number;
              kind: string;
              name: string;
            }>;
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
    // lives in the ☰ settings menu under an Export submenu and opens
    // the export dialog (Meshing Phase 0). Two-step flow: confirm the
    // cell size, click Mesh to run the tolerance-driven pass in the
    // mesher worker, then Save hands the bytes to the browser download.
    await page.getByTestId("settings-menu-trigger").click();
    await page.getByTestId("menu-export").hover();
    await page.getByTestId("menu-export-stl").click();
    await expect(page.getByTestId("export-dialog")).toBeVisible();
    // Step 1: no meshing yet — the method dropdown + cell size are shown;
    // confirm with the Mesh button.
    await expect(page.getByTestId("export-idle")).toBeVisible();
    await expect(page.getByTestId("export-method")).toBeVisible();
    await page.getByTestId("export-mesh").click();
    // Step 2: the export pass meshes at export resolution — allow it time.
    const saveButton = page.getByTestId("export-save");
    await expect(saveButton).toBeEnabled({ timeout: 60_000 });
    // The report must render with a verdict line.
    await expect(page.getByTestId("export-verdict")).toBeVisible();
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      saveButton.click(),
    ]);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const bytes = await fs.readFile(path);
    // Binary STL: 80-byte header + u32 triangle count + 50 bytes per
    // triangle. The parametric export path (Meshing Phase 2) reduces a
    // plain extruded rectangle to its exact 12 box triangles — so
    // assert structure + a real solid, not a magic byte size.
    expect(bytes.length).toBeGreaterThanOrEqual(84 + 12 * 50);
    const triCount = bytes.readUInt32LE(80);
    expect(triCount).toBeGreaterThanOrEqual(12);
    expect(bytes.length).toBe(84 + triCount * 50);

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
    await expandGeometrySection(page); // ticket 87a1: gated geometry list

    // Drag on the 2D overlay canvas. pointerdown → move → up
    // drives the new drag-to-draw path in SketchCanvas2d. We
    // don't assert resolved sketch-space coordinates because
    // those depend on the ortho camera zoom — we only care
    // that (a) one sub-spine entry is produced, (b) nothing
    // crashes.
    // Drive the drag through SKETCH coordinates (screen ≠ sketch axes since
    // the 3D rebuild). Deliberately off-axis in sketch space (Δu 80, Δv -55 ≈
    // 34°, clear of the H/V/parallel inference cone) and both endpoints clear
    // of the origin, so exactly ONE sub-call is produced: the line, with no
    // inferred orientation constraint.
    const project = await makeProjector(page);
    const [startX, startY] = await project(-40, 45);
    const [midX, midY] = await project(0, 17.5);
    const [endX, endY] = await project(40, -10);

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    // A couple of intermediate moves exercise the live-preview
    // path in SketchCanvas2d's RAF loop.
    await page.mouse.move(midX, midY, { steps: 5 });
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

  test("constraint tools live in the tool row and the Select tool toggles active", async ({
    page,
  }) => {
    // zz1r: the constraint buttons are sketch tools now — always present in
    // the drawing-tool row, no separate selection-gated palette. Picking a
    // drawing tool doesn't hide them; the Select tool just reads active.
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // The constraint tools are visible from the start, in select mode.
    await expect(page.getByTestId("sketch-constraint-parallel")).toBeVisible();
    await expect(
      page.getByTestId("sketch-constraint-horizontal"),
    ).toBeVisible();

    // Picking a drawing tool leaves them in place (they're tools too).
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expect(page.getByTestId("sketch-constraint-parallel")).toBeVisible();

    // The Select tool drops back to select mode and reads active.
    await page.getByTestId("sketch-tool-select").click();
    await expect(page.getByTestId("sketch-tool-select")).toHaveClass(
      /is-active/,
    );

    // Activating a constraint tool marks it active; clicking it again toggles
    // back to plain select mode.
    await page.getByTestId("sketch-constraint-horizontal").click();
    await expect(page.getByTestId("sketch-constraint-horizontal")).toHaveClass(
      /is-active/,
    );
    await page.getByTestId("sketch-constraint-horizontal").click();
    await expect(page.getByTestId("sketch-tool-select")).toHaveClass(
      /is-active/,
    );

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("pick two lines, apply Parallel, solver makes them parallel", async ({
    page,
  }) => {
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
    const [m1x, m1y] = await project(50, 35); // midpoint of line 1
    const [m2x, m2y] = await project(50, -20); // midpoint of line 2
    await page.mouse.click(m1x, m1y);
    await page.waitForTimeout(120); // let the store/overlay settle the pick
    await page.mouse.click(m2x, m2y);

    // The palette now offers Parallel + Perpendicular for two lines.
    await expect(page.getByTestId("sketch-constraint-parallel")).toBeVisible();
    await expect(
      page.getByTestId("sketch-constraint-perpendicular"),
    ).toBeVisible();

    // Apply Parallel — a third sub-spine row (the constraint) appears.
    await page.getByTestId("sketch-constraint-parallel").click();
    await expect(subSpineRows).toHaveCount(3);
    await page.waitForTimeout(800); // let the solver land

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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
    const dir = (l: {
      params: { start: [number, number]; end: [number, number] };
    }) => [
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

  test("pick two points, apply Coincident, solver merges them", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await expandGeometrySection(page); // ticket 87a1: gated tool dialog + list

    // Two free points a short distance apart, both off the origin.
    await pickSketchTool(page, "sketch_point");
    await page.getByLabel("At x").fill("40");
    await page.getByLabel("At y").fill("40");
    await page.getByTestId("sketch-tool-apply").click();
    await pickSketchTool(page, "sketch_point");
    await page.getByLabel("At x").fill("70");
    await page.getByLabel("At y").fill("55");
    await page.getByTestId("sketch-tool-apply").click();

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(2);

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [p1x, p1y] = await project(40, 40);
    const [p2x, p2y] = await project(70, 55);
    await page.mouse.click(p1x, p1y);
    await page.waitForTimeout(120); // let the store/overlay settle the pick
    await page.mouse.click(p2x, p2y);

    await expect(
      page.getByTestId("sketch-constraint-coincident"),
    ).toBeVisible();
    await page.getByTestId("sketch-constraint-coincident").click();
    await expect(subSpineRows).toHaveCount(3);
    await page.waitForTimeout(800);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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
    const selectionCount = () =>
      page.evaluate(() => {
        const store = (
          window as unknown as {
            __fdStore?: {
              state: { sketchSession: { selection: number[] } };
            };
          }
        ).__fdStore;
        return store?.state.sketchSession.selection.length ?? -1;
      });

    const [mx, my] = await project(50, 35);
    await page.mouse.click(mx, my);
    await page.waitForTimeout(120);
    // The line is selected.
    expect(await selectionCount()).toBe(1);

    // Click far-off empty space → the selection clears.
    const [ex, ey] = await project(-90, -90);
    await page.mouse.click(ex, ey);
    await page.waitForTimeout(120);
    expect(await selectionCount()).toBe(0);

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

  test("length dimension on a line drives its length when edited", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // A 40 mm horizontal line, off the origin.
    await drawLine(page, [10, 0], [50, 0]);

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [mx, my] = await project(30, 0); // line midpoint
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

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const dist = committed!.find(
      (c) => c.type === "sketch_constraint_distance",
    ) as { params: { value_mm: number } } | undefined;
    expect(dist).toBeDefined();
    expect(dist!.params.value_mm).toBeCloseTo(80, 3);

    // The solved line is actually 80 mm long.
    const line = committed!.find(
      (c) => c.type === "sketch_line",
    ) as unknown as {
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

  test("angle dimension drives two lines to the edited angle", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await drawLine(page, [10, 0], [50, 0]); // line A, horizontal
    await drawLine(page, [10, 20], [50, 40]); // line B, ~26.6°

    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [ax, ay] = await project(30, 0); // midpoint of line A
    const [bx, by] = await project(30, 30); // midpoint of line B
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

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const ang = committed!.find((c) => c.type === "sketch_constraint_angle") as
      { params: { value_deg: number } } | undefined;
    expect(ang).toBeDefined();
    expect(ang!.params.value_deg).toBeCloseTo(90, 3);

    // The two solved lines are perpendicular: direction dot product ~0.
    const lines = committed!.filter(
      (c) => c.type === "sketch_line",
    ) as unknown as Array<{
      params: { start: [number, number]; end: [number, number] };
    }>;
    expect(lines.length).toBe(2);
    const dir = (l: {
      params: { start: [number, number]; end: [number, number] };
    }) => [
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
                subCalls: Array<{
                  type: string;
                  params: Record<string, unknown>;
                }>;
              };
            };
          };
        }
      ).__fdStore;
      if (!store) return null;
      const s = store.getState().sketchSession;
      return {
        active: s.active,
        selection: [...s.selection],
        subCalls: s.subCalls,
      };
    });
  }

  test("drag a line endpoint to relocate it", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // A DIAGONAL line (10,8)→(50,32), off the origin. Deliberately off-axis:
    // a horizontal/vertical dialog line picks up an implicit H/V relationship
    // whose live solver then drags the far endpoint along when a grip moves —
    // here we want a pure single-DoF endpoint reshape, so the geometry must be
    // unconstrained.
    await drawLine(page, [10, 8], [50, 32]);

    // Select the line by clicking its midpoint. The palette's
    // single-line options confirm the line is selected (and so its
    // endpoint grips are now live).
    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [mx, my] = await project(30, 20);
    await page.mouse.click(mx, my);
    await expect(
      page.getByTestId("sketch-constraint-horizontal"),
    ).toBeVisible();

    // Grab the end-point grip at (50, 32) and drag it to (50, 60). The line is
    // unconstrained, so the solver reshapes only the dragged endpoint and
    // leaves the start put.
    const [ex, ey] = await project(50, 32);
    const [tx, ty] = await project(50, 60);
    await page.mouse.move(ex, ey);
    await page.mouse.down();
    await page.mouse.move((ex + tx) / 2, (ey + ty) / 2, { steps: 5 });
    await page.mouse.move(tx, ty, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const line = committed!.find(
      (c) => c.type === "sketch_line",
    ) as unknown as {
      params: { start: [number, number]; end: [number, number] };
    };
    expect(line).toBeDefined();
    // Start unchanged, end relocated to ~(50, 60) within pixel-rounding.
    expect(Math.abs(line.params.start[0] - 10)).toBeLessThan(2);
    expect(Math.abs(line.params.start[1] - 8)).toBeLessThan(2);
    expect(Math.abs(line.params.end[0] - 50)).toBeLessThan(2);
    expect(Math.abs(line.params.end[1] - 60)).toBeLessThan(2);

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
    await pickSketchTool(page, "sketch_circle");
    await expandGeometrySection(page); // ticket 87a1: gated tool dialog + apply
    await page.getByLabel("Center x").fill("40");
    await page.getByLabel("Center y").fill("0");
    await page.getByLabel("Radius").fill("20");
    await page.getByTestId("sketch-tool-apply").click();

    // Select the circle by clicking its rightmost rim point (60, 0).
    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [rimx, rimy] = await project(60, 0);
    await page.mouse.click(rimx, rimy);

    // Confirm the circle is selected so the rim grip is live.
    const sel = await readSketchSession(page);
    expect(sel).not.toBeNull();
    expect(sel!.selection.length).toBeGreaterThan(0);

    // Drag the rim grip from (60, 0) out to (80, 0): radius 20 → 40.
    const [tx, ty] = await project(80, 0);
    await page.mouse.move(rimx, rimy);
    await page.mouse.down();
    await page.mouse.move((rimx + tx) / 2, rimy, { steps: 5 });
    await page.mouse.move(tx, ty, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const circle = committed!.find(
      (c) => c.type === "sketch_circle",
    ) as unknown as {
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

  test("drag a rectangle corner grip to resize it, opposite corner fixed", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    // A precise rectangle (-20,-10)..(20,10) via the dialog.
    await pickSketchTool(page, "sketch_rectangle");
    await expandGeometrySection(page); // ticket 87a1: gated tool dialog + apply
    await page.getByLabel("Corner a x").fill("-20");
    await page.getByLabel("Corner a y").fill("-10");
    await page.getByLabel("Corner b x").fill("20");
    await page.getByLabel("Corner b y").fill("10");
    await page.getByTestId("sketch-tool-apply").click();

    // Select the rectangle by clicking its bottom edge midpoint
    // (0, -10): an edge maps to all four corner grips.
    await page.getByTestId("sketch-tool-select").click();
    const project = await makeProjector(page);
    const [ex, ey] = await project(0, -10);
    await page.mouse.click(ex, ey);
    const sel = await readSketchSession(page);
    expect(sel).not.toBeNull();
    expect(sel!.selection.length).toBeGreaterThan(0);

    // Grab the top-right corner grip (20, 10) and drag it to (40, 30).
    // The internal H/V constraints keep it axis-aligned; the opposite
    // corner (-20, -10) stays put.
    const [trx, try_] = await project(20, 10);
    const [tx, ty] = await project(40, 30);
    await page.mouse.move(trx, try_);
    await page.mouse.down();
    await page.mouse.move((trx + tx) / 2, (try_ + ty) / 2, { steps: 5 });
    await page.mouse.move(tx, ty, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(900);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    const committed = await readCommittedSubCalls(page);
    expect(committed).not.toBeNull();
    const rect = committed!.find(
      (c) => c.type === "sketch_rectangle",
    ) as unknown as {
      params: { corner_a: [number, number]; corner_b: [number, number] };
    };
    expect(rect).toBeDefined();
    const xs = [rect.params.corner_a[0], rect.params.corner_b[0]];
    const ys = [rect.params.corner_a[1], rect.params.corner_b[1]];
    // Opposite corner fixed at (-20,-10); dragged corner now ~(40,30).
    expect(Math.abs(Math.min(...xs) - -20)).toBeLessThan(2);
    expect(Math.abs(Math.min(...ys) - -10)).toBeLessThan(2);
    expect(Math.abs(Math.max(...xs) - 40)).toBeLessThan(2);
    expect(Math.abs(Math.max(...ys) - 30)).toBeLessThan(2);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
