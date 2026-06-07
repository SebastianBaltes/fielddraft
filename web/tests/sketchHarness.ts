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
  __fdSketchCamera?: () => {
    zoom: number;
    centerU: number;
    centerV: number;
    version: number;
  };
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

/** Wait until the ortho camera stops moving (publishes async + auto-fits). */
export async function settleSketchCamera(page: Page): Promise<void> {
  const read = () =>
    page.evaluate(() => {
      const c = (window as unknown as SketchStoreWindow).__fdSketchCamera?.();
      return c
        ? { zoom: c.zoom, centerU: c.centerU, centerV: c.centerV, version: c.version }
        : null;
    });
  let cam = await read();
  for (let i = 0; i < 50; i++) {
    await page.waitForTimeout(100);
    const next = await read();
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
  expect(cam, "sketch camera never published").not.toBeNull();
  expect(cam!.version).toBeGreaterThan(0);
}

/** Canvas-centre client pixel, for camera-independent draw gestures. */
export async function canvasCentre(page: Page): Promise<[number, number]> {
  const box = await page.getByTestId("sketch-canvas-2d").boundingBox();
  expect(box).not.toBeNull();
  return [box!.x + box!.width / 2, box!.y + box!.height / 2];
}

/** Project a sketch (u, v) to a client pixel IN-PAGE using the live rect +
 *  camera — the exact inverse of the app's `screenToSketch`. */
export async function sketchToClientPx(
  page: Page,
  u: number,
  v: number,
): Promise<[number, number]> {
  return page.evaluate(
    ({ u, v }) => {
      const cv = document.querySelector(
        '[data-testid="sketch-canvas-2d"]',
      ) as HTMLElement;
      const r = cv.getBoundingClientRect();
      const cam = (window as unknown as SketchStoreWindow).__fdSketchCamera!();
      return [
        r.left + r.width / 2 + (u - cam.centerU) * cam.zoom,
        r.top + r.height / 2 - (v - cam.centerV) * cam.zoom,
      ] as [number, number];
    },
    { u, v },
  );
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
export async function committedSketchTypes(page: Page): Promise<string[] | null> {
  return page.evaluate(() => {
    const kernel = (window as unknown as SketchStoreWindow).__fdKernel;
    if (!kernel) return null;
    const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
    if (!sketch) return null;
    return kernel.getSketchSubCalls(sketch.id).map((c) => c.type);
  });
}
