import { expect, type Page } from "@playwright/test";

/**
 * Shared helpers for real-GPU sketch E2Es.
 *
 * The 2D sketch overlay is a `<canvas data-testid="sketch-canvas-2d">` that
 * the app maps to sketch millimetres via its live `getBoundingClientRect()`
 * and the ortho-camera bridge published at `window.__fdSketchCamera`. Two
 * facts make naïve projection fragile:
 *
 *  1. The view AUTO-FITS after the first geometry lands, so any projector
 *     captured before drawing is stale.
 *  2. A point pick is unforgiving — an 8 px offset (e.g. a box measured a
 *     frame early) misses the vertex entirely, even though a curve pick at
 *     the same error still lands on the rim.
 *
 * So: DRAW with raw pixel offsets from the canvas centre (camera-independent,
 * always on-canvas), then SELECT by projecting the entity's ACTUAL committed
 * coordinate to a client pixel IN-PAGE using the live rect + live camera —
 * exactly the formula `screenToSketch`/`sketchToScreen` use. That projection
 * is computed fresh at click time, so it can't go stale.
 */

export interface SketchStoreWindow {
  __fdStore?: {
    state: {
      sketchSession: {
        selection: number[];
        selectedTool: string | null;
        subCalls: Array<{ type: string; params?: Record<string, unknown> }>;
      };
      /** F24: true once the selected glyph font is parsed and ready. */
      sketchTextFontReady?: boolean;
    };
  };
  __fdKernel?: {
    listFeatures: () => Array<{ id: number; kind: string }>;
    getSketchSubCalls: (id: number) => Array<{ type: string }>;
  };
  /** The live sketch camera. Mirrors `SketchCameraState` — note there is no
   *  `centerU`/`centerV`: since the sketch-in-3D rebuild the overlay projects
   *  through the real plane frame and camera, not a 2D pan/zoom pair. */
  __fdSketchCamera?: () => {
    zoom: number;
    version: number;
  };
  /** The app's OWN sketch→screen projection (canvas-local CSS px), i.e. the
   *  exact inverse of the overlay's hit-test. Tests must project with this
   *  rather than re-deriving it. */
  __fdSketchToScreen?: (u: number, v: number) => [number, number];
}

/** Clear persisted project state and start fresh. */
export async function freshLoad(page: Page): Promise<void> {
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
}

/**
 * Wait until the sketch camera has published a SETTLED, FINITE zoom — the app
 * mounts the WebGPU canvas first and auto-fits a frame later, so an early read
 * gives `version = 0` or a zoom of Infinity. Anything projected off that camera
 * comes out NaN.
 */
export async function settleSketchCamera(page: Page): Promise<void> {
  const read = () =>
    page.evaluate(() => {
      const c = (window as unknown as SketchStoreWindow).__fdSketchCamera?.();
      return c ? { zoom: c.zoom, version: c.version } : null;
    });
  let cam = await read();
  for (let i = 0; i < 50; i++) {
    await page.waitForTimeout(100);
    const next = await read();
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
  expect(cam, "sketch camera never published").not.toBeNull();
  expect(cam!.version).toBeGreaterThan(0);
  expect(
    Number.isFinite(cam!.zoom) && cam!.zoom > 0,
    `sketch camera zoom never became finite (got ${cam!.zoom})`,
  ).toBe(true);
}

/** Canvas-centre client pixel, for camera-independent draw gestures. */
export async function canvasCentre(page: Page): Promise<[number, number]> {
  const box = await page.getByTestId("sketch-canvas-2d").boundingBox();
  expect(box).not.toBeNull();
  return [box!.x + box!.width / 2, box!.y + box!.height / 2];
}

/** Project a sketch (u, v) to a client pixel IN-PAGE using the live rect +
 *  camera — the exact inverse of the app's `screenToSketch`.
 *
 * Projects through the app's OWN `__fdSketchToScreen`, and that is the whole
 * point. This helper used to re-derive the projection by hand as
 * `centre + (u - cam.centerU) * cam.zoom` — a flat 2D pan/zoom. Since the
 * sketch-in-3D rebuild the overlay instead lifts (u, v) into world space
 * through the active plane frame and projects with the live camera, and
 * `centerU`/`centerV` were dropped from the published camera altogether. The
 * hand-rolled formula therefore read `undefined`, every projection came out
 * NaN, and the click died inside Playwright as "Input.dispatchMouseEvent:
 * Invalid parameters" — nowhere near the actual cause. Commit 6b60445 already
 * learned this for `sketch-editor`'s local projector; the shared harness never
 * got the memo. Ask the app where a point is; don't guess. */
export async function sketchToClientPx(
  page: Page,
  u: number,
  v: number,
): Promise<[number, number]> {
  const px = await page.evaluate(
    ({ u, v }) => {
      const cv = document.querySelector(
        '[data-testid="sketch-canvas-2d"]',
      ) as HTMLElement;
      const r = cv.getBoundingClientRect();
      const toScreen = (window as unknown as SketchStoreWindow)
        .__fdSketchToScreen;
      if (!toScreen) return null;
      // Canvas-LOCAL CSS px — add the box origin for absolute client px.
      const [lx, ly] = toScreen(u, v);
      return [r.left + lx, r.top + ly] as [number, number];
    },
    { u, v },
  );
  expect(px, "__fdSketchToScreen hook missing (dev build only)").not.toBeNull();
  // Fail here, naming the culprit, rather than handing NaN to page.mouse and
  // getting "Input.dispatchMouseEvent: Invalid parameters" three calls later.
  expect(
    Number.isFinite(px![0]) && Number.isFinite(px![1]),
    `projection of sketch (${u}, ${v}) is not finite (got ${px![0]}, ${px![1]}) — camera not settled?`,
  ).toBe(true);
  return px!;
}

/** Click the committed sketch point at (u, v). Settles the camera first so
 *  the projection is current. Returns the resulting selection. */
export async function clickSketchPoint(
  page: Page,
  u: number,
  v: number,
): Promise<number[]> {
  await settleSketchCamera(page);
  const [px, py] = await sketchToClientPx(page, u, v);
  await page.mouse.click(px, py);
  await page.waitForTimeout(200);
  return page.evaluate(
    () =>
      (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
        .selection ?? [],
  );
}

/** The sketch tools that live inside a split-button flyout (ticket pqoe),
 *  mapped to their group's testid. Everything else is a flat tool button. */
const SKETCH_TOOL_GROUPS: Record<string, string> = {
  sketch_circle: "sketch-tool-circle-group",
  sketch_circle_3p: "sketch-tool-circle-group",
  sketch_ellipse: "sketch-tool-circle-group",
  sketch_rectangle: "sketch-tool-rectangle-group",
  sketch_rectangle_center: "sketch-tool-rectangle-group",
  sketch_rectangle_3p: "sketch-tool-rectangle-group",
};

function hasClass(cls: string | null, name: string): boolean {
  return (cls ?? "").split(/\s+/).includes(name);
}

/**
 * Pick a sketch tool. The two things this guards against are why it exists —
 * both are ways the toolbar stopped being a plain row of buttons:
 *
 *  1. IDEMPOTENCE (ticket daa8). The button is a TOGGLE: clicking the ACTIVE
 *     tool detaches it back to select mode — that is how a chained Line ends.
 *     A helper that clicks before every stroke therefore ARMS the tool for the
 *     first stroke and DISARMS it for the second, and that second drag lands in
 *     select mode and grip-drags the geometry it was supposed to draw next to.
 *     So: click only when the tool isn't already active.
 *
 *  2. THE FLYOUT (ticket pqoe). Circle/rectangle/ellipse variants are no longer
 *     flat buttons; they are rows in a split-button dropdown. The rows stay
 *     MOUNTED while the flyout is collapsed but are parked off-screen, so
 *     `getByTestId("sketch-tool-sketch_circle").click()` resolves and then hangs
 *     until the test times out — the element exists but never becomes clickable.
 *     So: open the flyout via its chevron first, then click the row.
 */
export async function pickSketchTool(page: Page, tool: string): Promise<void> {
  const groupId = SKETCH_TOOL_GROUPS[tool];
  if (!groupId) {
    const button = page.getByTestId(`sketch-tool-${tool}`);
    if (!hasClass(await button.getAttribute("class"), "is-active"))
      await button.click();
    return;
  }
  const group = page.getByTestId(groupId);
  const row = page.getByTestId(`sketch-tool-${tool}`);
  // The group's primary is `is-active` only while one of ITS members is the
  // selected tool; the row is `is-active` while it is the variant the group
  // currently fronts (which is also true from mere last-used memory). Both
  // together mean THIS tool is armed — clicking again would toggle it off.
  const armed =
    hasClass(
      await group.locator("button.tool.primary").getAttribute("class"),
      "is-active",
    ) && hasClass(await row.getAttribute("class"), "is-active");
  if (armed) return;
  await group.getByTestId(`${groupId}-chevron`).click();
  await row.click();
}

/**
 * Ticket 87a1: the geometry list, per-tool edit block, empty-state and the
 * constraint manager now live inside the "Geometry & Constraints" sub-accordion
 * (testid `geometry-section`, header `geometry-header`) which is COLLAPSED BY
 * DEFAULT. Call this once after entering the sketch / before interacting with
 * any of those inner elements. Idempotent: expands only when collapsed (detected
 * via the chevron glyph ▸ = collapsed, ▾ = expanded).
 */
export async function expandGeometrySection(page: Page): Promise<void> {
  const header = page.getByTestId("geometry-header");
  await expect(header).toBeVisible();
  const chevron = header.locator(".accordion-chevron");
  const glyph = (await chevron.textContent())?.trim();
  if (glyph === "▸") {
    await header.click();
    await expect(chevron).toHaveText("▾");
  }
}

/** The live session sub-call type list. */
export async function subCallTypes(page: Page): Promise<string[]> {
  return page.evaluate(
    () =>
      (
        window as unknown as SketchStoreWindow
      ).__fdStore?.state.sketchSession.subCalls.map((c) => c.type) ?? [],
  );
}

/** The committed sketch's sub-call types (after Finish). */
export async function committedSketchTypes(
  page: Page,
): Promise<string[] | null> {
  return page.evaluate(() => {
    const kernel = (window as unknown as SketchStoreWindow).__fdKernel;
    if (!kernel) return null;
    const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
    if (!sketch) return null;
    return kernel.getSketchSubCalls(sketch.id).map((c) => c.type);
  });
}
