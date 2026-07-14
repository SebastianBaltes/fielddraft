import { expect, test } from "@playwright/test";
import {
  freshLoad,
  settleSketchCamera,
  sketchToClientPx,
  subCallTypes,
  type SketchStoreWindow,
} from "./sketchHarness";

/**
 * V2: imported `sketch_path` curves are vertex-selectable & grip-draggable,
 * and SVG `<rect rx/ry>` rounded corners survive import as quad-arc corners.
 *
 * Imports an SVG with a rounded rect, enters sketch, asserts a `sketch_path`
 * sub-call exists carrying quad segments (the corner arcs). Then grip-drags
 * one path vertex (the contour start) and asserts its coordinate changed in
 * the sub-call (read via window.__fdStore). Cold first-paint on real WebGPU
 * can be slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

/** The first sketch_path's first contour `start` coordinate (or null). */

test.describe("Sketch path edit — rounded rect + vertex grip drag", () => {
  test("imports a rounded rect as quad-arc sketch_path and drags a vertex", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-mode-indicator").waitFor();

    const svg = `<svg xmlns="http://www.w3.org/2000/svg">
      <rect x="0" y="0" width="40" height="24" rx="6" ry="6"/>
    </svg>`;

    await page.getByTestId("sketch-import-input").setInputFiles({
      name: "rounded.svg",
      mimeType: "image/svg+xml",
      buffer: Buffer.from(svg),
    });

    // The rounded rect imports as ONE sketch_path (no sharp line chain).
    await expect.poll(async () => subCallTypes(page)).toContain("sketch_path");
    const types = await subCallTypes(page);
    expect(types.filter((t) => t === "sketch_path").length).toBe(1);

    // The corners survived as quad-bezier segments + 4 straight edges.
    const detail = await page.evaluate(() => {
      const calls =
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .subCalls ?? [];
      const path = calls.find((c) => c.type === "sketch_path");
      const contours =
        (path?.params?.contours as
          | Array<{ segments?: Array<{ kind?: string }>; closed?: boolean }>
          | undefined) ?? [];
      const segs = contours[0]?.segments ?? [];
      return {
        closed: contours[0]?.closed === true,
        quads: segs.filter((s) => s.kind === "quad").length,
        lines: segs.filter((s) => s.kind === "line").length,
      };
    });
    expect(detail.closed).toBe(true);
    expect(detail.quads).toBeGreaterThanOrEqual(4);
    expect(detail.lines).toBe(4);

    // The canvas renders without a present/draw crash.
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    // Grip-drag a path vertex. The assertion is "the PATH changed", not "this
    // one coordinate changed": several vertices coincide on a closed rounded
    // rect (the contour start and the final segment's `to` sit on the same
    // point), and the grip picker takes the nearest — so pinning the check to
    // `contours[0].start`, as this used to, asserts against a vertex the drag
    // may legitimately not have been holding.
    await settleSketchCamera(page);
    const pathCoords = () =>
      page.evaluate(() => {
        const calls =
          (window as unknown as SketchStoreWindow).__fdStore?.state
            .sketchSession.subCalls ?? [];
        const path = calls.find((c) => c.type === "sketch_path");
        const contours = (path?.params?.contours ?? []) as Array<{
          start?: [number, number];
          segments?: Array<{ ctrl?: [number, number]; to?: [number, number] }>;
        }>;
        const out: number[] = [];
        for (const c of contours) {
          if (c.start) out.push(...c.start);
          for (const seg of c.segments ?? []) {
            if (seg.ctrl) out.push(...seg.ctrl);
            if (seg.to) out.push(...seg.to);
          }
        }
        return out;
      });

    const before = await pathCoords();
    expect(before.length, "the path has vertices").toBeGreaterThan(0);

    // Grab the first vertex (the contour start) and drag it.
    const [sx, sy] = await sketchToClientPx(page, before[0], before[1]);
    await page.getByTestId("sketch-tool-select").click();
    await page.mouse.click(sx, sy);
    await page.waitForTimeout(300);
    const selection = await page.evaluate(
      () =>
        (window as unknown as SketchStoreWindow).__fdStore?.state.sketchSession
          .selection ?? [],
    );
    expect(selection.length, "clicking a vertex selects it").toBeGreaterThan(0);

    const [tx, ty] = [sx - 30, sy - 30];
    await page.mouse.move(sx, sy);
    await page.mouse.down();
    await page.mouse.move((sx + tx) / 2, (sy + ty) / 2, { steps: 5 });
    await page.mouse.move(tx, ty, { steps: 5 });
    await page.mouse.up();

    // The drop re-solves through WASM asynchronously — poll instead of sleeping.
    await expect
      .poll(
        async () => {
          const after = await pathCoords();
          if (after.length !== before.length) return true;
          return after.some((v, i) => Math.abs(v - before[i]) > 1e-3);
        },
        { timeout: 5000 },
      )
      .toBe(true);

    expect(pageErrors).toEqual([]);
  });
});
